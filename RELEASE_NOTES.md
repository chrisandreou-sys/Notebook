## Notebook RS v0.1.0

A local-first Rust research notebook with source-grounded chat and five creative exports.

**Downloads**
- **Debian/Ubuntu:** `notebook-rs_0.1.0_amd64.deb` — install with `sudo apt install ./notebook-rs_0.1.0_amd64.deb`, then run `notebook-rs`.
- **Linux portable:** `NotebookRS_0.1.0_x86_64.AppImage` — make executable and run. Some Linux systems require AppImage/FUSE support or `APPIMAGE_EXTRACT_AND_RUN=1`.
- **Windows 64-bit:** `notebook-rs_0.1.0_windows_x86_64.exe` — run directly, no installer required.

The app opens a localhost page in your browser. Create a notebook, upload or paste sources, then configure your own OpenAI-compatible chat model in **Settings**. Cloud, local (e.g. Ollama), and hybrid (local draft + cloud verification) modes are available. A compatible WAV speech endpoint is needed for spoken podcasts and narrated videos; otherwise podcasts are saved as transcripts and videos can be silent.

**Export formats:** standalone HTML slide decks, SVG infographics and mind maps, WAV podcasts, and MJPEG/PCM AVI videos. Image-only PDFs need OCR before upload. This is an independent Rust app, not Google's proprietary NotebookLM service; generated content depends on your configured model and should be fact-checked.

Source code, privacy notes, supported formats, and build instructions are in the [README](https://github.com/chrisandreou-sys/Notebook#readme).
