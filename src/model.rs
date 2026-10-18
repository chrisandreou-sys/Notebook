use std::{
    env, fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_UPLOAD: usize = 20 * 1024 * 1024;
pub const MAX_TEXT_CHARS: usize = 400_000;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Endpoint {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub primary: Endpoint,
    pub secondary: Endpoint,
    /// "cloud", "local", or "hybrid". Hybrid asks the secondary model for a
    /// draft and the primary model to verify it against the retrieved sources.
    pub mode: String,
    pub tts: Endpoint,
    pub voice_a: String,
    pub voice_b: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            primary: Endpoint {
                base_url: "https://api.openai.com/v1".into(),
                model: "gpt-4o-mini".into(),
                api_key: String::new(),
            },
            secondary: Endpoint {
                base_url: "http://127.0.0.1:11434/v1".into(),
                model: String::new(),
                api_key: String::new(),
            },
            mode: "cloud".into(),
            tts: Endpoint::default(),
            voice_a: "alloy".into(),
            voice_b: "nova".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Chunk {
    pub index: usize,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub text: String,
    pub chunks: Vec<Chunk>,
    pub upload_file: Option<String>,
    pub truncated: bool,
    pub created_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Citation {
    pub number: usize,
    pub source_id: String,
    pub source_name: String,
    pub chunk_index: usize,
    pub excerpt: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Exchange {
    pub question: String,
    pub answer: String,
    pub citations: Vec<Citation>,
    pub created_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub kind: String,
    pub title: String,
    /// Filename under the private artifacts directory. No user-controlled paths.
    pub file_name: String,
    pub description: String,
    /// Serialized rendering plan for the on-screen preview and transcript.
    pub plan: String,
    pub created_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Notebook {
    pub id: String,
    pub title: String,
    pub sources: Vec<Source>,
    pub exchanges: Vec<Exchange>,
    pub artifacts: Vec<Artifact>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl Notebook {
    pub fn new(title: &str) -> Self {
        let now = timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            title: title.trim().chars().take(100).collect(),
            sources: Vec::new(),
            exchanges: Vec::new(),
            artifacts: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Database {
    pub settings: Settings,
    pub notebooks: Vec<Notebook>,
}

pub struct Store {
    pub root: PathBuf,
    pub db: Database,
}

impl Store {
    pub fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        }
        fs::create_dir_all(root.join("uploads"))?;
        fs::create_dir_all(root.join("artifacts"))?;
        let db_file = root.join("notebooks.json");
        let db = if db_file.exists() {
            serde_json::from_slice(&fs::read(&db_file)?)
                .with_context(|| format!("reading {} (your data has not been overwritten)", db_file.display()))?
        } else {
            Database::default()
        };
        Ok(Self { root, db })
    }

    pub fn save(&self) -> Result<()> {
        let tmp = self.root.join(format!(".notebooks-{}.tmp", Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut file = private_file(&tmp)?;
            serde_json::to_writer(&mut file, &self.db)?;
            file.flush()?;
            file.sync_all()?;
            let dest = self.root.join("notebooks.json");
            #[cfg(windows)]
            if dest.exists() {
                // Windows does not replace an existing destination with rename.
                // Keep a backup until the new file has been installed.
                let backup = self.root.join("notebooks.json.bak");
                if backup.exists() { fs::remove_file(&backup)?; }
                fs::rename(&dest, &backup)?;
                if let Err(e) = fs::rename(&tmp, &dest) {
                    let _ = fs::rename(&backup, &dest);
                    return Err(e.into());
                }
                let _ = fs::remove_file(backup);
                return Ok(());
            }
            fs::rename(&tmp, &dest)?;
            Ok(())
        })();
        if result.is_err() { let _ = fs::remove_file(tmp); }
        result
    }

    pub fn notebook(&self, id: &str) -> Option<&Notebook> {
        self.db.notebooks.iter().find(|n| n.id == id)
    }

    pub fn notebook_mut(&mut self, id: &str) -> Option<&mut Notebook> {
        self.db.notebooks.iter_mut().find(|n| n.id == id)
    }

    pub fn upload_path(&self, name: &str) -> PathBuf { self.root.join("uploads").join(name) }
    pub fn artifact_path(&self, name: &str) -> PathBuf { self.root.join("artifacts").join(name) }
}

pub fn private_file(path: &Path) -> Result<fs::File> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    Ok(opts.open(path)?)
}

pub fn default_data_dir() -> PathBuf {
    if let Some(dir) = env::var_os("XDG_DATA_HOME") { return PathBuf::from(dir).join("notebook-rs"); }
    #[cfg(windows)]
    if let Some(dir) = env::var_os("APPDATA") { return PathBuf::from(dir).join("NotebookRS"); }
    if let Some(home) = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) {
        let home = PathBuf::from(home);
        #[cfg(target_os = "macos")]
        { return home.join("Library/Application Support/NotebookRS"); }
        #[cfg(not(target_os = "macos"))]
        { return home.join(".local/share/notebook-rs"); }
    }
    PathBuf::from("notebook-rs-data")
}

pub fn timestamp() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

pub fn clean_name(name: &str) -> String {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = name.chars().filter(|c| !c.is_control()).take(120).collect();
    if cleaned.trim().is_empty() { "Untitled source".into() } else { cleaned }
}

pub fn clipped(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

pub fn source_from_text(name: &str, text: &str, upload_file: Option<String>) -> Result<Source> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let original_count = text.chars().count();
    let text = clipped(text.trim(), MAX_TEXT_CHARS);
    if text.trim().is_empty() { bail!("No searchable text was found. Scanned PDFs require OCR before upload."); }
    let chunks = crate::retrieval::chunk_text(&text);
    Ok(Source {
        id: Uuid::new_v4().to_string(),
        name: clean_name(name), text, chunks, upload_file,
        truncated: original_count > MAX_TEXT_CHARS,
        created_at: timestamp(),
    })
}

pub fn extract_file(filename: &str, bytes: &[u8]) -> Result<String> {
    if bytes.len() > MAX_UPLOAD { bail!("File exceeds the 20 MB upload limit."); }
    let filename = clean_name(filename);
    let extension = Path::new(&filename).extension().and_then(|s| s.to_str())
        .unwrap_or("").to_ascii_lowercase();
    let text = match extension.as_str() {
        "txt" | "md" | "markdown" | "csv" | "tsv" | "json" | "yaml" | "yml" | "log" | "srt" | "vtt" => {
            String::from_utf8_lossy(bytes).into_owned()
        }
        "html" | "htm" | "xml" => strip_markup(&String::from_utf8_lossy(bytes)),
        "pdf" => pdf_extract::extract_text_from_mem(bytes).context("reading PDF text")?,
        "docx" => extract_zip_xml(bytes, "word/document.xml", false)?,
        "pptx" => extract_zip_xml(bytes, "ppt/slides/slide", true)?,
        "epub" => extract_zip_xml(bytes, "", true)?,
        _ => bail!("Unsupported file type. Upload PDF, DOCX, PPTX, EPUB, TXT, Markdown, CSV, HTML, JSON or XML."),
    };
    if text.trim().is_empty() { bail!("No searchable text was found. Scanned PDFs require OCR before upload."); }
    Ok(text)
}

fn extract_zip_xml(bytes: &[u8], prefix: &str, multiple: bool) -> Result<String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).context("reading document archive")?;
    let mut names = Vec::new();
    for i in 0..archive.len() {
        let name = archive.by_index(i)?.name().to_owned();
        let match_name = if prefix.is_empty() {
            name.ends_with(".xhtml") || name.ends_with(".html")
        } else if multiple {
            name.starts_with(prefix) && name.ends_with(".xml") && !name.contains("_rels")
        } else { name == prefix };
        if match_name { names.push(name); }
    }
    if names.is_empty() { bail!("The document contains no readable text parts."); }
    names.sort_by_key(|name| natural_slide_number(name));
    let mut result = String::new();
    for name in names {
        let mut part = archive.by_name(&name)?;
        // Defend against highly compressed ZIP bombs.
        if part.size() > 8 * 1024 * 1024 { bail!("Document text part is too large."); }
        let mut xml = String::new();
        part.read_to_string(&mut xml).context("reading document text")?;
        result.push_str(&strip_markup(&xml));
        result.push_str("\n\n");
        if !multiple || result.len() > MAX_TEXT_CHARS * 4 { break; }
    }
    Ok(result)
}

fn natural_slide_number(name: &str) -> usize {
    name.trim_end_matches(".xml").trim_start_matches("ppt/slides/slide")
        .parse().unwrap_or(0)
}

/// Plain text extraction for HTML and Office Open XML. No HTML is rendered or executed.
pub fn strip_markup(xml: &str) -> String {
    let mut result = String::new();
    let mut tag = String::new();
    let mut in_tag = false;
    let mut ignore: Option<String> = None;
    for ch in xml.chars() {
        if ch == '<' {
            in_tag = true;
            tag.clear();
        } else if ch == '>' && in_tag {
            in_tag = false;
            let name = tag.trim().split_whitespace().next().unwrap_or("").trim_end_matches('/');
            let closing = name.starts_with('/');
            let clean = name.trim_start_matches('/');
            let simple = clean.rsplit(':').next().unwrap_or(clean).to_ascii_lowercase();
            if simple == "script" || simple == "style" {
                if closing { ignore = None; } else { ignore = Some(simple.clone()); }
            }
            if ignore.is_none() && (closing && matches!(simple.as_str(), "p" | "div" | "h1" | "h2" | "h3" | "li" | "tr") || matches!(simple.as_str(), "br" | "tab")) {
                result.push('\n');
            }
        } else if in_tag {
            tag.push(ch);
        } else if ignore.is_none() {
            result.push(ch);
        }
    }
    decode_entities(&result).lines().map(str::trim).filter(|s| !s.is_empty())
        .collect::<Vec<_>>().join("\n")
}

fn decode_entities(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        if let Some(end) = rest.find(';').filter(|end| *end <= 12) {
            let entity = &rest[1..end];
            let decoded = match entity {
                "amp" => Some('&'), "lt" => Some('<'), "gt" => Some('>'),
                "quot" => Some('"'), "apos" | "#39" => Some('\''), "nbsp" => Some(' '),
                _ if entity.starts_with("#x") => u32::from_str_radix(&entity[2..], 16).ok().and_then(char::from_u32),
                _ if entity.starts_with('#') => entity[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            if let Some(ch) = decoded {
                out.push(ch);
                rest = &rest[end + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &rest[1..];
    }
    out.push_str(rest);
    out
}

pub fn safe_extension(filename: &str) -> Result<String> {
    let extension = Path::new(filename).extension().and_then(|s| s.to_str())
        .unwrap_or("").to_ascii_lowercase();
    if !matches!(extension.as_str(), "pdf" | "docx" | "pptx" | "epub" | "txt" | "md" | "markdown" | "csv" | "tsv" | "json" | "yaml" | "yml" | "log" | "srt" | "vtt" | "html" | "htm" | "xml") {
        return Err(anyhow!("Unsupported file type"));
    }
    Ok(extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn office_and_html_text_is_plain() {
        assert_eq!(strip_markup("<p>Hello &amp; welcome</p><script>bad()</script><p>42</p>"), "Hello & welcome\n42");
        assert_eq!(strip_markup("<w:p><w:r><w:t>One</w:t></w:r></w:p><w:p><w:t>Two</w:t></w:p>"), "One\nTwo");
    }

    #[test]
    fn store_roundtrip_and_private_names() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().join("data")).unwrap();
        store.db.notebooks.push(Notebook::new("Research"));
        store.save().unwrap();
        assert_eq!(Store::open(store.root.clone()).unwrap().db.notebooks[0].title, "Research");
        assert_eq!(clean_name("../foo\\bar.pdf"), "bar.pdf");
    }

    #[test]
    fn docx_upload_extracts_paragraphs() {
        use std::io::Write;
        let buffer = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(buffer);
        writer.start_file("word/document.xml", zip::write::FileOptions::default()).unwrap();
        writer.write_all(b"<w:document><w:p><w:r><w:t>First</w:t></w:r></w:p><w:p><w:t>Second</w:t></w:p></w:document>").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        assert_eq!(extract_file("notes.docx", &bytes).unwrap(), "First\nSecond");
    }

    #[test]
    fn empty_source_is_rejected() {
        assert!(source_from_text("empty", "  \n", None).is_err());
    }
}
