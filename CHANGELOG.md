# Changelog

Tutte le modifiche rilevanti per gli utenti saranno documentate in questo file.

Il formato segue [Keep a Changelog](https://keepachangelog.com/it/1.1.0/) e il
progetto aderisce a [Semantic Versioning](https://semver.org/lang/it/).

## [Unreleased]

### Added

- Contesto di progetto, decisioni architetturali e roadmap della v1.
- Documentazione del setup BlackHole per macOS.
- Politica iniziale di versioning, privacy e qualità.
- Bootstrap Rust con contratti di dominio, schemi di sessione e logging locale.
- CLI iniziale con i comandi `devices`, `doctor`, `start`, `status`, `pause`,
  `resume`, `stop` ed `export`.
- Simulatore realtime da WAV con normalizzazione mono 16 kHz e persistenza audio
  incrementale.
- Controllo tra processi di stato, pausa, ripresa e stop tramite file locali
  atomici, con metriche della coda bounded.
- Segmentazione vocale deterministica con soglia RMS configurabile, isteresi,
  preroll/postroll, silenzio finale e limite massimo di durata.
- Worker e coda bounded dedicati alla segmentazione, con metriche osservabili e
  flush dei segmenti pendenti durante pausa e arresto.
- Trascrizione locale italiana tramite whisper.cpp, accelerata con Metal e
  configurabile dalla CLI.
- Transcript canonico verbatim con identità SHA-256 del modello, parametri
  d'inferenza e metriche ASR persistite per sessione.
- Benchmark ASR riproducibile con fixture non sensibili, WER/CER, real-time factor,
  picco RSS e report di riferimento per Apple M5.
- Export derivati dal transcript canonico nei formati JSONL, testo, Markdown, SRT
  e VTT, con scrittura atomica e protezione dalla sovrascrittura della fonte.
- Enumerazione dei dispositivi CoreAudio e diagnostica locale di piattaforma,
  modello, spazio disco, input audio e disponibilità di BlackHole.
- Report `status` operativo con metadati, percorsi di output e ritardo ASR.
- Validazione preventiva di percorsi, lingua, configurazioni e spazio disponibile.
- Finalizzazione recuperabile delle sessioni interrotte da SIGINT o SIGTERM, con
  stato ed evento di errore espliciti.
- Cattura live macOS da microfono e BlackHole su stream separati, con resampling
  mono 16 kHz, mix attenuato e persistenza delle due tracce originali.
- Metriche live di segnale, picco, clock drift e blocchi persi, più probe audio
  esplicito tramite `doctor --probe-audio`.
- Limiti di memoria sulle code e sui buffer di cattura; overflow, stallo,
  disconnessione e cambi di formato falliscono esplicitamente senza perdita muta.
- Lock advisory recuperabili dopo crash, recovery esplicito delle sessioni rimaste
  attive e serializzazione dei comandi di controllo concorrenti.
- Percorsi di sessione assoluti e validazione di contenimento prima di operazioni
  di controllo.
- Protezione di tutti gli artefatti canonici da sovrascritture tramite export.
- Limiti espliciti di memoria e durata per il simulatore WAV in-memory.
