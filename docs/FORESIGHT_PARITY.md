# Foresight Parity — Contesto e decisioni

> **Istruzioni per l'agente (LLM):** questo file è la **fonte di verità del contesto** per il progetto "Foresight Parity". Leggilo integralmente prima di qualsiasi task su questo branch. Non contraddire la sezione **Vincoli**. Quando prendi una decisione, aggiungila al **Registro decisioni** con la data. Quando risolvi una **Domanda aperta**, spostala nel registro. Quando scrivi un'affermazione tecnica, allega l'evidenza (§5.4).

---

## 0. Metadati

- **Progetto:** Foresight Parity
- **Repo:** `FabioTognaa/Personal-Transcriber` — locale `~/Desktop/Programmazione/personal-transcriber`
- **Branch di lavoro:** `dev/foresight-parity` (creato da `main` @ `cb0eef2`)
- **Data di avvio:** 2026-10-14
- **Stato:** avvio. Solo contesto e decisioni. **Nessuna riga di codice scritta.**
- **Owner:** Fabio Tognarelli

## 1. Scopo

Capire **come funziona** Google AI Edge Foresight (architettura, tecniche, pipeline) e portare nel `personal-transcriber` le feature e le tecniche che ci mancano.

**Non è scopo:** copiare codice sorgente o pesi di Google; dipendere dalla suite/servizi Google; ridisegnare la UI (rinviato — §3 V3).

Il valore atteso non è il prodotto in sé ma la **comprensione delle tecniche** + un possibile tool distribuibile su GitHub (portfolio).

## 2. I due sistemi (baseline)

### 2.1 personal-transcriber (nostro)

- **Linguaggio:** Rust. ~42k LOC. CLI. 20 moduli in `src/`.
- **ASR:** whisper.cpp / ggml — `models/ggml-small-q5_1.bin`, `models/ggml-large-v3-turbo-q8_0.bin`.
- **Modalità:** trascrizione live (mic + audio di sistema via BlackHole) e batch (mp3, m4a, mp4, aac).
- **Storage:** `sessions/<id>/` con metadati + trascrizione progressiva; export `.txt` / `.md` / `.json`.
- **Controllo:** due terminali (start / control).
- **Limiti noti (dal README):** nessun VAD (soglia RMS), nessuna diarizzazione, solo IT/EN, nessun cloud, nessuna UI, sessioni non cifrate at-rest.
- **Test:** `tests/*.rs`. **Benchmark:** `benchmarks/`.

### 2.2 Foresight (target di studio)

- **Forma:** app GUI **Flutter macOS** + **Launch Agent** in background (`ForesightHelper`).
- **Runtime modelli:** **LiteRT-LM** (`libLiteRt.dylib` + accelerator Metal/WebGPU); file modello in formato `.litertlm`.
- **Modelli on-device scaricati:** Gemma 4 E4B (3,4 GB) + modelli di supporto; un modello di embedding a 768 dimensioni. Config in `on_device_model_configure.json` (varianti offerte: Gemma 4 12B / E4B / E2B; tutte con visione + audio).
- **LLM:** Gemma on-device + fallback Gemini cloud; toggle "Privacy Mode" (100% on-device).
- **Cattura:** riunioni (mic + sistema) con overlay; import batch (file, Google Drive, URL).
- **Feature utente:** trascrizione live, speaker assignment, insight, note, daily briefing, meeting brief, action items, creazione eventi calendario, voice command.
- **Conoscenza:** RAG su embedding 768d + **knowledge graph** (nodi/archi/cluster) + reranking ("V2 Retrieval").
- **Storage:** SQLite (`database.sqlite`, `key_value_store.sqlite`, `rag_vector.db`, `agent_metadata.db`) + protobuf (`app_settings.pb`, `.mfca/state/sessions.pb`).
- **Integrazioni:** Google sign-in (OAuth), Drive, Calendar, Firebase, GoogleUpdater.
- **Dati:** `~/Library/Application Support/com.google.AIEdgeForesight/` (~6,9 GB).
- **App:** `/Applications/Foresight.app`, versione `1.0.0` (build `1.0.0.28.20261005`).

## 3. Vincoli (HARD — non violare)

- **V1 — Indipendenza.** Nessuna dipendenza runtime dalla suite/servizi Google nel prodotto finale. Ogni feature va replicata con componenti aperti/terzi (o nostri).
- **V2 — Contenuto nel repo.** Mappa, evidenze, spec e decisioni vivono in questo repo, branch `dev/foresight-parity`.
- **V3 — UI rinviata.** Nessun lavoro di UI ora. Alcune feature la richiederanno → se ne discute separatamente.
- **V4 — Doc per LLM.** I documenti sono scritti per essere letti/usati da un agente: strutturati, espliciti, non ambigui.
- **V5 — Incrementale.** Refactor grande, ma consegnato a ticket piccoli.
- **V6 — Clean room.** Tecniche e idee sì; codice o pesi Google nel prodotto no.

## 4. Registro decisioni

| # | Data | Decisione | Note |
|---|------|-----------|------|
| D1 | 2026-10-14 | Progetto "Foresight Parity" sul repo `personal-transcriber`; branch `dev/foresight-parity` creata da `main`. | Branch creata. |
| D2 | 2026-10-14 | Profondità RE: **prima black-box** (feature + comportamento), **poi deep** (binario/DB/protocolli) solo dove serve per implementare. | "Opzione 1 con riserva". |
| D3 | 2026-10-14 | Metodo RE: adottare il workflow agentico di **Matt Pocock** (`mattpocock/skills`) + usare **REA** (`morluto/rea`) come toolkit di RE. | Da installare/valutare. |
| D4 | 2026-10-14 | Obiettivo finale: migliorare il nostro software; possibile tool distribuibile su GitHub (portfolio). | Non un gioco. |
| D5 | 2026-10-14 | I modelli di Foresight **non** vengono riusati: si usano modelli aperti ufficiali su runtime aperti. | Analisi in §8. |

## 5. Metodo di lavoro

### 5.1 Workflow Pocock (5 fasi)

`setup → allineamento (grill) → piano (spec/ticket/mappa) → build → miglioramento`

- Setup: `setup-matt-pocock-skills` (1× per repo; scrive `docs/agents/`, issue tracker, label di triage).
- Allineamento: `grill-with-docs` (interrogatorio + **linguaggio di dominio** → `GLOSSARY.md`, ADR).
- Pianificazione: `wayfinder` (mappa enorme come decision-ticket), `research` (indagini su fonti primarie, output markdown citato, in background), `to-spec`, `to-tickets`.
- Build: `tdd`, `implement`, `code-review`.
- Miglioramento: `retro`, `improve-codebase-architecture`.
- **Install:** `npx skills@latest add mattpocock/skills`. Già presenti in `~/.agents/skills`: `grill-me`, `grill-with-docs`, `grilling`, `handoff`, `to-questionnaire`, `wayfinder`.

### 5.2 REA — Reverse Engineer Anything

- Repo **`morluto/rea`** (MIT), sito `rea.tools`. Toolkit **MCP + agent-skills** che collegano l'agente a tool di analisi (Ghidra, Hopper, disassembler, analisi app/Electron/APK/firmware/WASM…).
- Flusso: **decompile → understand → recreate**.
- Da valutare: compatibilità con il nostro harness (OpenCode), modalità di installazione, skill incluse.
- Ruolo nel progetto: **coprire il "buco"** del metodo di RE (le skill di Pocock sono orientate al codice, non al RE).

### 5.3 Toolkit tecnico

- **Statico:** `strings`, `otool`/`nm`, **blutter** (Dart AOT), Ghidra/Hopper, `sqlite3`, decode protobuf, `tar`.
- **Dinamico:** `lsof` / `fs_usage` / `dtruss`, `sample` / `spindump`, `mitmproxy`, Frida.
- **Black-box:** sessioni UI scriptate + cattura evidenze (screen recording, probing dei prompt).

### 5.4 Regola evidenza

Ogni affermazione tecnica nella mappa deve avere **evidenza riproducibile**: comando + output + hash, salvata in `docs/evidence/`. La mappa senza evidenza è un'opinione.

## 6. Deliverable (mappa)

| Livello | Doc | Contenuto |
|---|---|---|
| L0 | `docs/evidence/00-inventario.md` | Bundle, framework, helper, asset, DB, protobuf, modelli |
| L1 | `docs/10-features.md` | Feature map black-box + flussi utente |
| L2 | `docs/20-architettura.md` | Processi, IPC (unix socket helper), storage, data model |
| L3 | `docs/30-pipeline.md` | audio→ASR→insight; ingestion→embedding→grafo→retrieval; prompt |
| L4 | `docs/40-runtime.md` | Varianti modelli, quantizzazione, speculative decoding, KV cache, backend |
| L5 | `docs/50-integrazioni.md` | Drive/Calendar/OAuth, Firebase, download modelli, updater |
| L6 | `docs/60-nonfunzionale.md` | RAM/thermal, modalità privacy, agente background, perf |
| L7 | `docs/70-delta.md` | Delta matrix Foresight vs personal-transcriber (+ colonna "replica senza Google") |

Ogni doc: *cosa fa · come funziona (evidenza) · confidenza · come replicarlo nel nostro stack · gap*.

## 7. Domande aperte

- **Q1.** Possiamo riusare i `.litertlm` installati da Foresight nel nostro transcriber? → **Risposta preliminare: no, non conviene.** Analisi in §8. Da confermare.
- **Q2.** REA è compatibile con il nostro harness e utile nel flusso Pocock? Da installare/valutare.
- **Q3.** Con cosa replichiamo, senza Google, i modelli di: ASR, embedding, LLM (per insight/riassunti)?

## 8. Modelli: analisi della domanda Q1

**Fatti verificati**

- I file modello di Foresight sono **non cifrati** e mappabili in memoria → tecnicamente copiabili.
- Il formato `.litertlm` è il bundle di **LiteRT-LM** (Google AI Edge); runtime `libLiteRt*.dylib`.
- Il nostro stack è **Rust + whisper.cpp (ggml)**: formati e runtime **incompatibili**.
- Alcuni modelli di Foresight (es. ASR "TinyGemmaAsr", "expert models") sono probabilmente **interni / non pubblici**.
- `EmbeddingGemma` (768d) e la linea Gemma "E2B/E4B" sono **pubblici** — verificare la disponibilità della variante "Gemma 4".

**Conclusione preliminare**

1. Riusare i file di Foresight **non è la strada**: aggiungerebbe un secondo runtime C++ (LiteRT-LM) a un progetto Rust/ggml, senza binding Rust maturi.
2. Via corretta: usare i **modelli aperti ufficiali** (pesi da Hugging Face / Kaggle) su **runtime aperti con binding Rust** — llama.cpp / Candle / ONNX Runtime — oppure alternative non-Google.
3. ASR: si resta su **whisper.cpp**. Embedding/LLM: valutare modelli aperti indipendenti.
4. **Legale:** i pesi Gemma sono sotto **Gemma Terms** (non Apache). Prelevarli dall'installazione di un'altra app ≠ via pulita. Verificare la licenza **per singolo modello**.

## 9. Glossario (dominio)

- **ASR** — Automatic Speech Recognition: voce → testo.
- **VAD** — Voice Activity Detection: rileva la presenza di parlato.
- **Diarizzazione** — attribuzione dei segmenti agli speaker. Foresight: speaker assignment via ASR. Noi: assente.
- **Embedding** — vettore a D dimensioni che rappresenta un testo. Foresight: 768.
- **RAG** — Retrieval-Augmented Generation: generazione ancorata a documenti recuperati.
- **Knowledge graph** — grafo di entità e relazioni estratte dai documenti.
- **LiteRT-LM** — runtime LLM on-device di Google AI Edge; formato `.litertlm`.
- **whisper.cpp / ggml** — runtime ASR locale che già usiamo.
- **Clean room** — reimplementare dal comportamento osservato, senza copiare codice.

## 10. Prossimi passi

1. Installare/valutare il workflow Pocock (§5.1) e REA (§5.2). — *decisione rimandata all'utente.*
2. Produrre i primi 3 doc: glossario, **L0 inventario**, **L7 delta matrix**.
3. Poi scendere su L1 (feature map).

## 11. NON fare (ora)

- Non installare nulla, non toccare `Foresight.app`, non avviare il RE, finché non deciso.
- Non scrivere UI.
- Non introdurre dipendenze Google.
- Non copiare pesi o codice Google.
