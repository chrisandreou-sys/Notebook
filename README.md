# Notebook RS

A **local-first research notebook and AI studio**, written in Rust. Upload sources, ask source-grounded questions with passage citations, and create presentations, infographics, mind maps, podcasts, and videos with an AI provider you configure. The UI is served by the Rust executable in your browser; there is no JavaScript, CDN, Python service, or hosted notebook account.

> Notebook RS is an independent application, **not a copy of Google's proprietary NotebookLM service or models**. Its generated outputs depend on the capabilities of the configured AI and speech provider. It does not reproduce NotebookLM's private prompts, voices, video generation models, or Google integrations.

## Quick start

Download the `.deb`, `.AppImage`, or Windows `.exe` from [Releases](https://github.com/chrisandreou-sys/Notebook/releases). Or build from source with a current stable Rust toolchain:

```sh
cargo run --release
# Visit http://127.0.0.1:3040 (opens automatically on a desktop)
```

The Debian package installs `notebook-rs` on your PATH. On Linux, make the AppImage executable (`chmod +x NotebookRS_*.AppImage`) and open it. On Windows, run the `.exe`. The app opens your default browser and listens **only on localhost** unless you explicitly choose otherwise.

1. Create a notebook and add PDF, DOCX, PPTX, EPUB, TXT, Markdown, CSV, HTML, JSON, XML, or pasted text sources. Files are limited to 20 MB each, five per upload. Extracted searchable text is capped at 400,000 characters per source. Scanned/image-only PDFs need OCR before upload.
2. Open **Settings**. Set a primary OpenAI-compatible API base URL (including `/v1`), chat model, and API key when required. For Ollama, use `http://127.0.0.1:11434/v1` as the secondary base URL and the name of a model you have installed.
3. Pick **Cloud** (primary), **Local** (secondary), or **Hybrid** (secondary drafts, primary verifies against the source excerpts). Retrieval/indexing always happens locally; selected excerpts are sent only to the chosen provider(s) when generating.
4. Ask a question or create a studio piece. Answers link `[1]` citations to the corresponding passages. A question with no matching passage is declined locally rather than sent to the AI.

### Studio outputs

| Tool | Download | Details |
| --- | --- | --- |
| Slide deck | Standalone `.html` | Full-screen, print-friendly presentation with previous/next links; works offline without scripts. |
| Infographic | `.svg` | Resolution-independent visual summary; safe, escaped labels. |
| Mind map | `.svg` | Connected branches and child concepts. |
| Podcast | `.wav` | Two-voice, AI-scripted audio when `/audio/speech` returns 16-bit PCM WAV. If TTS is unavailable, a clearly labeled `.html` transcript is saved instead. |
| Video overview | `.avi` | Rust-rendered MJPEG video with optional PCM narration when the speech endpoint is available. The AVI downloads for desktop playback; not all browsers play AVI inline. |

The Studio uses actual source excerpts and asks the model not to invent unsupported facts. **Generated text still needs human verification.** Audio requires a provider with an OpenAI-compatible speech endpoint; setting a chat-only local model will not magically add speech synthesis. A silent video can still be created when speech is unavailable.

## Privacy and configuration

Notebook files, extracted text, artifacts, and API settings are stored under `$XDG_DATA_HOME/notebook-rs` (or `~/.local/share/notebook-rs`), `%APPDATA%\NotebookRS` on Windows, or `~/Library/Application Support/NotebookRS` on macOS. Override with `NOTEBOOK_DATA_DIR` or `--data-dir PATH`. On Unix, the app sets the data directory to `0700` and new data files to `0600`. API keys are stored **locally in plaintext, protected by filesystem permissions**, not in a cloud account or browser storage. Back up or remove that directory as needed. Do not expose the server to other users on an untrusted network; the app has no accounts or network authentication. Forms use a per-process anti-CSRF token, and the site has a restrictive Content Security Policy.

```sh
notebook-rs --help
notebook-rs --host 127.0.0.1 --port 3040 --no-open
notebook-rs --data-dir /path/to/private/library
```

To connect from another device or a browser preview you can pass `--host 0.0.0.0`, but put authentication in front of it first. The browser uses same-origin, relative links; it never calls an AI endpoint directly. AI credentials are sent by the Rust server over HTTP(S) only to the configured provider. For remote providers use HTTPS.

## Development and packaging

```sh
cargo fmt --check
cargo test
cargo build --release
bash scripts/package-linux.sh 0.1.0  # .deb and .AppImage on Linux
```

The [build workflow](.github/workflows/build.yml) tests/builds on Ubuntu and Windows, packages Linux releases, and uploads a Windows `.exe`. The application code and media encoders are Rust; the release script is only a packaging helper. Fonts embedded for video rendering are DejaVu Sans; see [`assets/FONT-LICENSE.txt`](assets/FONT-LICENSE.txt). No `ffmpeg`, LibreOffice, Node, Python, JavaScript frontend, or remote rendering backend is required.

### Current boundaries

- Text extraction is not OCR; scanned PDFs and images are not searchable until converted to text.
- Retrieval uses on-device BM25-style lexical ranking with source diversification. It does not use NotebookLM's proprietary search index or embedding model.
- AI provider compatibility assumes OpenAI-style `/chat/completions`; speech assumes `/audio/speech` supporting WAV. Models and provider quotas vary. The app makes no AI calls before you configure it.
- HTML slide decks and AVI videos are portable exports, not PowerPoint `.pptx` or H.264 `.mp4` files. AVI is best played in a desktop player.

This project is licensed under MIT (the embedded font has its own notice).
