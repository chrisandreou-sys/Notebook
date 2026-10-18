use std::collections::{HashMap, HashSet};

use crate::model::{clipped, Citation, Source};

const CHUNK_SIZE: usize = 1_100;
const OVERLAP: usize = 130;

pub fn chunk_text(text: &str) -> Vec<crate::model::Chunk> {
    let chars: Vec<char> = text.chars().collect();
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = (start + CHUNK_SIZE).min(chars.len());
        if end < chars.len() {
            let min = start + CHUNK_SIZE / 2;
            if let Some(pos) = (min..end).rev().find(|&p| chars[p] == '\n') {
                end = pos + 1;
            } else if let Some(pos) = (min..end).rev().find(|&p| chars[p].is_whitespace()) {
                end = pos + 1;
            }
        }
        let content: String = chars[start..end].iter().collect();
        if !content.trim().is_empty() {
            chunks.push(crate::model::Chunk {
                index: chunks.len(),
                text: content.trim().to_owned(),
            });
        }
        if end == chars.len() {
            break;
        }
        start = end.saturating_sub(OVERLAP);
    }
    chunks
}

fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| {
            t.chars().count() > 1
                && !matches!(
                    *t,
                    "a" | "an"
                        | "and"
                        | "are"
                        | "as"
                        | "at"
                        | "be"
                        | "by"
                        | "can"
                        | "do"
                        | "does"
                        | "for"
                        | "from"
                        | "how"
                        | "in"
                        | "is"
                        | "it"
                        | "of"
                        | "on"
                        | "or"
                        | "the"
                        | "their"
                        | "this"
                        | "to"
                        | "was"
                        | "were"
                        | "what"
                        | "when"
                        | "where"
                        | "which"
                        | "who"
                        | "why"
                        | "with"
                        | "would"
                        | "you"
                        | "about"
                        | "please"
                        | "tell"
                        | "me"
                )
        })
        .map(str::to_owned)
        .collect()
}

fn general_question(question: &str) -> bool {
    let q = question.to_lowercase();
    [
        "summar",
        "overview",
        "key points",
        "main ideas",
        "takeaways",
        "outline",
        "tldr",
        "tl;dr",
        "explain this",
        "what is this",
        "describe this",
    ]
    .iter()
    .any(|word| q.contains(word))
}

/// BM25 search over local chunks. Citations are always real stored excerpts; the
/// model never gets to invent source IDs. At most two passages per source are
/// selected in the first pass to avoid drowning out smaller documents.
pub fn retrieve(sources: &[Source], question: &str, limit: usize) -> Vec<Citation> {
    let docs: Vec<(&Source, &crate::model::Chunk)> = sources
        .iter()
        .flat_map(|source| source.chunks.iter().map(move |chunk| (source, chunk)))
        .collect();
    if docs.is_empty() || limit == 0 {
        return Vec::new();
    }

    let query: HashSet<String> = tokens(question).into_iter().collect();
    let document_tokens: Vec<Vec<String>> =
        docs.iter().map(|(_, chunk)| tokens(&chunk.text)).collect();
    let avg_len = document_tokens.iter().map(Vec::len).sum::<usize>() as f64 / docs.len() as f64;
    let mut df = HashMap::new();
    for terms in &document_tokens {
        let distinct: HashSet<&String> = terms.iter().filter(|t| query.contains(*t)).collect();
        for term in distinct {
            *df.entry(term.clone()).or_insert(0usize) += 1;
        }
    }
    let mut scores: Vec<(usize, f64)> = document_tokens
        .iter()
        .enumerate()
        .map(|(index, terms)| {
            let counts: HashMap<&str, usize> = terms.iter().fold(HashMap::new(), |mut acc, t| {
                *acc.entry(t.as_str()).or_insert(0) += 1;
                acc
            });
            let score: f64 = query
                .iter()
                .map(|term| {
                    let freq = *counts.get(term.as_str()).unwrap_or(&0) as f64;
                    let containing = *df.get(term).unwrap_or(&0) as f64;
                    let idf =
                        (1.0 + (docs.len() as f64 - containing + 0.5) / (containing + 0.5)).ln();
                    let denom = freq + 1.2 * (0.25 + 0.75 * terms.len() as f64 / avg_len.max(1.0));
                    let title_boost = if tokens(&docs[index].0.name).contains(term) {
                        1.3
                    } else {
                        1.0
                    };
                    if freq == 0.0 {
                        0.0
                    } else {
                        idf * freq * 2.2 / denom * title_boost
                    }
                })
                .sum();
            (index, score)
        })
        .collect();
    scores.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let fallback = general_question(question);
    if fallback && scores.first().is_none_or(|(_, score)| *score <= 0.0) {
        // A request like "summarize" should draw from the beginning of *each*
        // source, not return an unrelated-chunks error.
        scores.sort_by_key(|(i, _)| docs[*i].1.index);
    } else {
        scores.retain(|(_, score)| *score > 0.0);
    }
    let mut chosen = Vec::new();
    let mut per_source: HashMap<&str, usize> = HashMap::new();
    for (index, _) in &scores {
        let (source, _) = docs[*index];
        if *per_source.get(source.id.as_str()).unwrap_or(&0) >= 2 {
            continue;
        }
        chosen.push(*index);
        *per_source.entry(&source.id).or_insert(0) += 1;
        if chosen.len() == limit {
            break;
        }
    }
    if chosen.len() < limit {
        for (index, _) in scores {
            if !chosen.contains(&index) {
                chosen.push(index);
            }
            if chosen.len() == limit {
                break;
            }
        }
    }
    chosen
        .into_iter()
        .enumerate()
        .map(|(n, index)| {
            let (source, chunk) = docs[index];
            Citation {
                number: n + 1,
                source_id: source.id.clone(),
                source_name: source.name.clone(),
                chunk_index: chunk.index,
                excerpt: chunk.text.clone(),
            }
        })
        .collect()
}

pub fn question_context(citations: &[Citation]) -> String {
    citations
        .iter()
        .map(|citation| {
            format!(
                "[{}] {} (passage {})\n{}",
                citation.number,
                citation.source_name,
                citation.chunk_index + 1,
                citation.excerpt
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}

/// Bounded, round-robin context sampling for generative studio tools. Artifacts
/// always depend on the uploaded material, rather than only the notebook title.
pub fn studio_context(sources: &[Source]) -> String {
    let mut output = String::new();
    let mut round = 0;
    while output.chars().count() < 17_000 && round < 10 {
        let mut added = false;
        for (i, source) in sources.iter().enumerate() {
            if let Some(chunk) = source.chunks.get(round) {
                let entry = format!(
                    "[Source {}: {} · passage {}]\n{}\n\n",
                    i + 1,
                    source.name,
                    round + 1,
                    clipped(&chunk.text, 950)
                );
                if output.chars().count() + entry.chars().count() > 18_000 {
                    break;
                }
                output.push_str(&entry);
                added = true;
            }
        }
        if !added {
            break;
        }
        round += 1;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::source_from_text;

    #[test]
    fn chunk_unicode_without_panic_or_lost_tail() {
        let input = "αβγ ".repeat(800);
        let chunks = chunk_text(&input);
        assert!(chunks.len() > 1);
        assert!(chunks.last().unwrap().text.ends_with("αβγ"));
    }

    #[test]
    fn retrieval_is_grounded_and_numbered() {
        let sources = vec![
            source_from_text(
                "Orbits",
                "The moon orbits the Earth in roughly 27 days.",
                None,
            )
            .unwrap(),
            source_from_text("Soil", "Tomatoes thrive in compost-rich soil.", None).unwrap(),
        ];
        let found = retrieve(&sources, "How long does the moon orbit take?", 4);
        assert_eq!(found[0].source_name, "Orbits");
        assert_eq!(found[0].number, 1);
        assert!(!found.iter().any(|c| c.source_name == "Soil"));
        assert!(retrieve(&sources, "quantum entanglement", 4).is_empty());
    }

    #[test]
    fn overview_falls_back_to_actual_sources() {
        let sources = vec![source_from_text("Memo", "An important finding.", None).unwrap()];
        assert_eq!(retrieve(&sources, "Summarize everything", 3).len(), 1);
    }
}
