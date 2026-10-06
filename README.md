# live-transcript

CLI locale per registrare e trascrivere riunioni in corso. La prima versione è
destinata a macOS Apple Silicon, usa BlackHole per leggere l'audio della call e
conserva audio e trascrizioni esclusivamente sul computer dell'utente.

Il progetto è in sviluppo iniziale. Può simulare una sessione riproducendo un file
WAV in tempo reale e segmentare localmente il parlato con un VAD energetico
deterministico. Cattura live e ASR non sono ancora implementati.

## Principi

- Nessuna API cloud, telemetria o upload di audio.
- Trascrizione ASR non rielaborata da LLM, correttori o riassuntori.
- Registrazione audio progressiva e transcript con timestamp.
- CLI prima di qualsiasi interfaccia grafica.
- Core Rust portabile; adattatore di cattura macOS nella prima versione.

## Stato previsto della prima versione

La v1 registrerà una traccia mixata di microfono e audio remoto, trascrivendola in
italiano con segmenti finali dopo una pausa. Il comando `pause` continuerà a
registrare l'audio, ma sospenderà la trascrizione.

Windows, Linux, inglese, rilevamento automatico lingua, diarizzazione degli speaker
e UI non fanno parte della v1.

## Simulatore WAV

`start` resta in foreground e normalizza il WAV in PCM mono 16 kHz, salvandolo in
una nuova directory di sessione:

```sh
cargo run -- start --input-wav tests/fixtures/m1_stream.wav
```

Da un secondo terminale, usando la stessa `--sessions-dir` se diversa dal valore
predefinito `sessions`, si può controllare la sessione:

```sh
cargo run -- status
cargo run -- pause
cargo run -- resume
cargo run -- stop
```

La pausa riguarda solo la segmentazione e la futura trascrizione: l'audio continua
a essere scritto. Il segmento pendente viene chiuso quando inizia la pausa e
l'elaborazione riparte senza attraversare l'intervallo sospeso.

Il VAD usa chunk mono 16 kHz, soglia RMS, isteresi di avvio, preroll/postroll,
silenzio finale e durata massima. Le soglie sono configurabili tramite le opzioni
`--vad-*` mostrate da `start --help`; i valori effettivi sono salvati in
`session.json`. Coda e metriche di segmentazione sono visibili in `status.json`.

In M2 `transcript.jsonl` resta intenzionalmente vuoto perché non esiste ancora un
motore ASR. I comandi `devices`, `doctor` ed `export` restano stub.

## Documentazione

- [Contesto completo e scelte architetturali](PROJECT_CONTEXT.md)
- [Roadmap di implementazione](ROADMAP.md)
- [Setup BlackHole su macOS](docs/BLACKHOLE_MACOS.md)
- [Storico delle modifiche](CHANGELOG.md)

## Sviluppo locale

Il progetto richiede Rust 1.85 o successivo. I controlli della milestone corrente
sono:

```sh
cargo test
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

## Privacy e consenso

Prima di registrare o trascrivere una riunione, informa i partecipanti e verifica le
politiche aziendali e gli obblighi applicabili. Il fatto che l'elaborazione sia locale
non elimina questo requisito.

## Licenza

MIT. Vedi [LICENSE](LICENSE).
