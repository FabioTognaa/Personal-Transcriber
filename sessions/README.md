# Sessioni

Ogni `start` crea una cartella qui. Il nome è l'ora locale di inizio:

```text
YYYY-MM-DD_HH-MM-SS
```

Esempio: `2026-10-07_01-17-03`. L'ordinamento alfabetico coincide con quello
cronologico. Se due sessioni partono nello stesso secondo, la seconda diventa
`2026-10-07_01-17-03_2`. L'identificatore stabile resta l'UUID in `session.json`,
non il nome della cartella.

Le registrazioni e i transcript restano sul disco locale e non vanno nel
repository. Questo README è l'unico file della directory tracciato da git.

## Layout di una sessione

```text
sessions/2026-10-07_01-17-03/
  session.json          metadati: UUID, stato, orari, lingua, modello, VAD, ASR
  status.json           fotografia runtime (coda, posizione audio, metriche)
  events.jsonl          avvio, pause, riprese, stop, errori
  transcript.jsonl      segmenti finali, una riga JSON ciascuno
  control.json          ultimo comando pause/resume/stop richiesto
  audio/mixed.wav       mix mono 16 kHz passato alla trascrizione
  audio/microphone.wav  solo cattura live: microfono
  audio/system.wav      solo cattura live: audio di sistema (BlackHole)
```

`transcript.jsonl` è la fonte macchina: testo, timestamp relativi all'audio,
lingua, identità del modello e parametri di inferenza. Per leggerlo come testo:

```sh
cargo run -- export sessions/2026-10-07_01-17-03 --format markdown
cargo run -- export sessions/2026-10-07_01-17-03 --format text
```

## File nella directory `sessions/`, non nella cartella della sessione

- `current-session.json` punta alla sessione attiva o all'ultima recuperata.
- `.runner.lock` impedisce un secondo processo di trascrizione in parallelo.
- `.control.lock` serializza `pause`, `resume` e `stop`.

I lock sono vuoti di proposito: il contenuto è il lock del sistema operativo, non
un documento da aprire.
