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
