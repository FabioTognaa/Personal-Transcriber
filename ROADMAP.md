# Roadmap di implementazione

Questa roadmap è l'ordine di lavoro vincolante per la v1. Ogni milestone termina
solo dopo che i relativi criteri di uscita sono verificati. Non si anticipa la
cattura live finché il core non funziona su audio riproducibile.

## Regole di esecuzione

- Un singolo filone di implementazione alla volta per il core della pipeline.
- Ogni milestone deve produrre codice, test e documentazione aggiornati.
- Una milestone bloccata da un vincolo tecnico produce una decisione documentata,
  non una soluzione provvisoria nascosta.
- Ogni commit usa Conventional Commits; modifiche utente-visibili aggiornano il
  changelog.
- Prima di una release: test verdi, `git diff --check`, changelog e versione
  coerenti.

## M0 — Bootstrap e contratti

**Stato:** completata il 6 ottobre 2026.

**Obiettivo:** creare un progetto Rust compilabile che definisca i confini del
dominio, senza acquisizione audio o ASR.

**Implementazione**

- Inizializzare `Cargo.toml`, struttura moduli e dipendenze minime.
- Implementare il parsing CLI con i comandi previsti, inizialmente come stub
  verificabili.
- Definire i tipi di dominio: sessione, chunk PCM, sorgente, segmento,
  evento, stato della sessione e configurazione.
- Definire gli schemi JSONL e la directory di una sessione.
- Introdurre logging strutturato locale e messaggi di errore utilizzabili da CLI.
- Aggiungere formattazione, lint e test unitari al workflow locale.

**Criteri di uscita**

- `cargo test`, `cargo fmt --check` e `cargo clippy -- -D warnings` passano.
- `--help` e i comandi CLI espongono opzioni coerenti, anche se non ancora operative.
- Gli schemi di evento e transcript hanno test di serializzazione.
- Non esistono dipendenze cloud né I/O audio implicito.

## M1 — Sessioni, eventi e simulatore di stream

**Stato:** completata il 6 ottobre 2026.

**Obiettivo:** dimostrare il ciclo di vita completo di una sessione usando un WAV
locale come sorgente, letto a velocità reale.

**Implementazione**

- Creare e finalizzare directory di sessione.
- Scrivere `session.json`, `events.jsonl` e `transcript.jsonl` append-only.
- Implementare `FileAudioSource`, che normalizza WAV in chunk PCM temporizzati.
- Implementare code bounded e metriche di backlog.
- Gestire `start`, `status`, `pause`, `resume` e `stop` sul simulatore.
- Durante `pause`, continuare a scrivere l'audio ma non produrre segmenti ASR.

**Criteri di uscita**

- Una fixture WAV produce timestamp monotoni e nessuna perdita di chunk.
- Stop e riavvio non corrompono i file già scritti.
- Pause/resume producono eventi espliciti e rispettano la semantica concordata.
- Un test d'integrazione copre una sessione intera simulata.

## M2 — Segmentazione vocale

**Obiettivo:** trasformare lo stream PCM in segmenti di parlato riproducibili.

**Implementazione**

- Introdurre un VAD iniziale e configurabile.
- Definire soglie per inizio, fine e durata massima del segmento.
- Aggiungere preroll/postroll per non troncare fonemi ai bordi.
- Salvare metriche di durata, silenzio e segmenti scartati.

**Criteri di uscita**

- Fixture con silenzi e parlato producono confini ragionevoli e deterministici.
- Una pausa breve non spezza inutilmente una frase.
- Un intervento lungo viene segmentato senza perdita audio.
- Il VAD non blocca la sorgente né la persistenza audio.

## M3 — Trascrizione locale e benchmark

**Obiettivo:** integrare `whisper.cpp` e scegliere un modello italiano sostenibile
sull'hardware Apple Silicon target.

**Implementazione**

- Integrare `whisper.cpp` mediante binding Rust controllati.
- Rendere espliciti modello, lingua italiana e parametri d'inferenza.
- Convertire segmenti VAD in eventi transcript finali.
- Salvare modello e configurazione su ogni sessione.
- Aggiungere un benchmark riproducibile separato dai test.
- Valutare latenza, real-time factor e qualità su audio non sensibile.

**Criteri di uscita**

- Il sistema produce transcript JSONL locale per una sessione simulata.
- Nessun testo è modificato da correttori o LLM dopo l'ASR.
- Il real-time factor è inferiore a 1 con margine sul Mac target.
- Il modello scelto, la dimensione e il metodo d'installazione sono documentati.

## M4 — Export e operatività CLI

**Obiettivo:** rendere utile una sessione completata senza introdurre UI.

**Implementazione**

- Implementare export in testo semplice, Markdown, SRT e VTT a partire da JSONL.
- Completare `devices`, `doctor` e `status`.
- Aggiungere validazione dei percorsi, spazio disco, modello e configurazione.
- Rendere `stop` sicuro in caso di segnali di terminazione del processo.

**Criteri di uscita**

- Tutti gli export derivano dallo stesso transcript canonico.
- `doctor` individua precondizioni mancanti senza cambiare configurazioni macOS.
- Una sessione interrotta lascia dati leggibili e segnala correttamente l'errore.

## M5 — Cattura live macOS

**Obiettivo:** sostituire la sorgente file con microfono e BlackHole, preservando il
core invariato.

**Implementazione**

- Integrare enumerazione dispositivi e cattura tramite `cpal`.
- Acquisire microfono e BlackHole su due stream separati.
- Normalizzare, riallineare in modo basilare e mixare le tracce.
- Registrare tracce originali e mix.
- Implementare un test esplicito di segnale in `doctor`.
- Aggiornare la guida BlackHole con comportamento realmente verificato.

**Criteri di uscita**

- Una call di prova produce audio locale e remoto udibile nei file di sessione.
- Il tool mantiene l'acquisizione anche se l'ASR accumula ritardo.
- Nessun callback audio esegue inferenza o I/O bloccante.
- La CLI segnala dispositivi disconnessi e assenza di segnale.

## M6 — Hardening e prerelease

**Obiettivo:** rendere la v1 utilizzabile per call reali di durata significativa.

**Implementazione**

- Test end-to-end con sessioni lunghe e pause.
- Validazione di recovery dopo crash e terminazione forzata.
- Misurazione di memory growth, backlog e clock drift.
- GitHub Actions per format, lint, test e build macOS.
- Aggiornamento di README, changelog e istruzioni di installazione.

**Criteri di uscita**

- Sessione di prova reale completata senza perdita non segnalata.
- Test, lint e build passano in CI.
- Limiti noti e setup richiesto sono documentati chiaramente.
- Versione `0.1.0-alpha.1` pronta per tag e release privata.

## Dopo la v1

L'ordine previsto è: supporto inglese, rilevamento automatico lingua, adattatori
Windows/Linux, poi eventuale diarizzazione. UI, bot di meeting e funzionalità cloud
restano fuori scope finché non esiste una motivazione di prodotto esplicita.
