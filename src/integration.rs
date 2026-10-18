//! Full HTTP smoke test with a local, fake OpenAI-compatible API. No network
//! credentials or external services are needed for the test suite.

use std::sync::{atomic::{AtomicUsize, Ordering}, Arc, Mutex};

use axum::{extract::State, routing::post, Json, Router};
use reqwest::{header, redirect::Policy};
use serde_json::{json, Value};

use crate::{media::Audio, model::{Store, Settings}, web::{self, AppState}};

#[tokio::test]
async fn notebook_workflow_from_upload_to_all_five_studio_exports() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mock = Router::new()
        .route("/v1/chat/completions", post(mock_chat))
        .route("/v1/audio/speech", post(mock_speech))
        .with_state(calls.clone());
    let ai_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ai_port = ai_listener.local_addr().unwrap().port();
    let ai_handle = tokio::spawn(async move { axum::serve(ai_listener, mock).await.unwrap() });

    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path().join("library")).unwrap();
    let endpoint = format!("http://127.0.0.1:{ai_port}/v1");
    store.db.settings = Settings::default();
    store.db.settings.primary.base_url = endpoint.clone();
    store.db.settings.primary.model = "mock-chat".into();
    store.db.settings.secondary.base_url = endpoint.clone();
    store.db.settings.secondary.model = "mock-local".into();
    store.db.settings.tts.base_url = endpoint;
    store.db.settings.tts.model = "mock-voice".into();
    store.save().unwrap();
    let state = Arc::new(AppState { store: Mutex::new(store), client: reqwest::Client::new(), csrf: "test-csrf-token".into() });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = web::router(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::builder().redirect(Policy::none()).build().unwrap();
    let csrf = "test-csrf-token";

    let homepage = client.get(&base).send().await.unwrap().text().await.unwrap();
    assert!(homepage.contains("New notebook"));
    assert!(homepage.contains("name=\"csrf\""));
    let created = client.post(format!("{base}/notebooks"))
        .form(&[("csrf", csrf), ("title", "Lunar Research")]).send().await.unwrap();
    assert_eq!(created.status(), 303);
    let location = created.headers().get(header::LOCATION).unwrap().to_str().unwrap().to_owned();
    let id = location.split('/').nth(2).unwrap().split('?').next().unwrap();

    let pasted = client.post(format!("{base}/notebooks/{id}/sources/text"))
        .form(&[("csrf", csrf), ("title", "Orbit notes"), ("text", "The moon orbits Earth in roughly 27 days. The observation is repeated in the field journal.")])
        .send().await.unwrap();
    assert_eq!(pasted.status(), 303);

    // Browser-style multipart request: both the token and the original file
    // must be handled, and the original is downloadable after indexing.
    let boundary = "rust-test-boundary";
    let body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"csrf\"\r\n\r\n{csrf}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"observations.txt\"\r\nContent-Type: text/plain\r\n\r\nMoonlight data supports the orbit observations.\r\n--{boundary}--\r\n");
    let uploaded = client.post(format!("{base}/notebooks/{id}/sources/upload"))
        .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
        .body(body).send().await.unwrap();
    assert_eq!(uploaded.status(), 303);

    let settings_html = client.get(format!("{base}/settings")).send().await.unwrap().text().await.unwrap();
    assert!(settings_html.contains("Hybrid"));
    let settings = client.post(format!("{base}/settings")).form(&[
        ("csrf", csrf), ("mode", "hybrid"), ("primary_url", &format!("http://127.0.0.1:{ai_port}/v1")),
        ("primary_model", "mock-chat"), ("primary_key", "top-secret-test-key"),
        ("secondary_url", &format!("http://127.0.0.1:{ai_port}/v1")), ("secondary_model", "mock-local"),
        ("secondary_key", ""), ("tts_url", &format!("http://127.0.0.1:{ai_port}/v1")),
        ("tts_model", "mock-voice"), ("tts_key", ""), ("voice_a", "alloy"), ("voice_b", "nova")
    ]).send().await.unwrap();
    assert_eq!(settings.status(), 303);
    let settings_html = client.get(format!("{base}/settings")).send().await.unwrap().text().await.unwrap();
    assert!(!settings_html.contains("top-secret-test-key"));

    let asked = client.post(format!("{base}/notebooks/{id}/ask"))
        .form(&[("csrf", csrf), ("question", "How long does the moon orbit take?")]).send().await.unwrap();
    assert_eq!(asked.status(), 303);
    assert!(calls.load(Ordering::SeqCst) >= 2, "hybrid mode must call both models");
    let notebook_html = client.get(format!("{base}/notebooks/{id}"))
        .send().await.unwrap().text().await.unwrap();
    assert!(notebook_html.contains("roughly 27 days"));
    assert!(notebook_html.contains("class=\"citation-marker\""));
    assert!(notebook_html.contains("observations.txt"));

    for kind in ["slides", "infographic", "mindmap", "podcast", "video"] {
        let created = client.post(format!("{base}/notebooks/{id}/studio/{kind}"))
            .form(&[("csrf", csrf)]).send().await.unwrap();
        let status = created.status();
        if status != 303 { panic!("{kind} creation failed ({status}): {}", created.text().await.unwrap()); }
    }
    let (source_id, artifacts) = {
        let store = state.store.lock().unwrap();
        let notebook = store.notebook(id).unwrap();
        (notebook.sources[1].id.clone(), notebook.artifacts.clone())
    };
    assert_eq!(artifacts.len(), 5);
    for artifact in &artifacts {
        let download = client.get(format!("{base}/notebooks/{id}/artifacts/{}/download", artifact.id)).send().await.unwrap();
        assert_eq!(download.status(), 200);
        assert!(download.headers().contains_key(header::CONTENT_DISPOSITION));
        let bytes = download.bytes().await.unwrap();
        match artifact.kind.as_str() {
            "slides" => assert!(String::from_utf8_lossy(&bytes).contains("Start presentation")),
            "infographic" | "mindmap" => assert!(String::from_utf8_lossy(&bytes).contains("</svg>")),
            "podcast" => assert_eq!(&bytes[..4], b"RIFF"),
            "video" => assert_eq!(&bytes[8..12], b"AVI "),
            _ => unreachable!(),
        }
    }
    let original = client.get(format!("{base}/notebooks/{id}/sources/{source_id}/download"))
        .send().await.unwrap().text().await.unwrap();
    assert_eq!(original, "Moonlight data supports the orbit observations.");

    let forbidden = client.post(format!("{base}/notebooks/{id}/delete"))
        .form(&[("csrf", "incorrect")]).send().await.unwrap();
    assert_eq!(forbidden.status(), 400);
    assert_eq!(client.get(format!("{base}/notebooks/{id}/artifacts/missing"))
        .send().await.unwrap().status(), 404);
    server.abort();
    ai_handle.abort();
}

async fn mock_chat(State(calls): State<Arc<AtomicUsize>>, Json(payload): Json<Value>) -> Json<Value> {
    calls.fetch_add(1, Ordering::SeqCst);
    let user = payload["messages"][1]["content"].as_str().unwrap_or("");
    let answer = if user.contains("5–7 slide") {
        json!({"title":"Moon overview","subtitle":"Evidence from the notebook","slides":[
            {"title":"Orbit observations","bullets":["A roughly 27-day orbit","Repeated in field notes"],"narration":"The journal records a roughly twenty seven day orbit."},
            {"title":"Field observations","bullets":["Researchers recorded moonlight"],"narration":"Observations back the main finding."}
        ]}).to_string()
    } else if user.contains("polished infographic") {
        json!({"title":"Lunar facts","subtitle":"From our sources","facts":[{"label":"Orbit","value":"27 days","detail":"A repeated journal observation."}]}).to_string()
    } else if user.contains("useful mind map") {
        json!({"title":"The Moon","branches":[{"label":"Orbit","children":["Around Earth","Roughly 27 days"]}]}).to_string()
    } else if user.contains("two-host audio discussion") {
        json!({"title":"Lunar conversation","turns":[{"speaker":"A","text":"What did the journal record?"},{"speaker":"B","text":"The moon orbits Earth in roughly 27 days."}]}).to_string()
    } else {
        "The moon takes roughly 27 days to orbit Earth [1].".into()
    };
    Json(json!({"choices":[{"message":{"content":answer}}]}))
}
async fn mock_speech() -> ([(header::HeaderName, &'static str); 1], Vec<u8>) {
    let wav = Audio { rate: 24_000, samples: vec![0i16; 12_000] }.to_wav().unwrap();
    ([(header::CONTENT_TYPE, "audio/wav")], wav)
}
