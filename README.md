# live-transcript

CLI locale per registrare e trascrivere riunioni in corso. La prima versione è
destinata a macOS Apple Silicon, usa BlackHole per leggere l'audio della call e
conserva audio e trascrizioni esclusivamente sul computer dell'utente.

Il progetto è in sviluppo iniziale. Può acquisire microfono e BlackHole su stream
separati oppure simulare una sessione da WAV, segmentare localmente il parlato e
trascriverlo con whisper.cpp e accelerazione Metal. La cattura live richiede ancora
la verifica end-to-end su una call reale prima di considerare completata la milestone.

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

## Cattura live

Installare e configurare BlackHole come descritto nella
[guida macOS](docs/BLACKHOLE_MACOS.md), quindi verificare i nomi esatti:

```sh
cargo run -- devices
cargo run -- doctor --probe-audio
```

Durante il probe di tre secondi occorre parlare e riprodurre audio verso BlackHole.
Il probe apre entrambi gli input, verifica permessi, segnale e integrità delle code,
ma non registra una sessione. Per selezionare dispositivi non predefiniti:

```sh
cargo run -- doctor --probe-audio \
  --microphone "Microfono MacBook Air" \
  --system-audio "BlackHole 2ch"
```

Avviare la cattura live omettendo `--input-wav`:

```sh
cargo run --release -- start \
  --model models/ggml-small-q5_1.bin \
  --microphone "Microfono MacBook Air" \
  --system-audio "BlackHole 2ch"
```

La sessione conserva `audio/microphone.wav`, `audio/system.wav` e
`audio/mixed.wav`. I callback CoreAudio copiano soltanto i campioni in una coda
bounded: resampling, mix, disco, VAD e ASR avvengono fuori dal callback. Overflow,
disconnessione, cambio di sample rate o oltre cinque secondi di audio non abbinato
arrestano la sessione come fallita invece di perdere dati senza segnalarlo.
`status.json` espone picchi, presenza segnale, drift e blocchi callback persi.

## Simulatore WAV

Scaricare il modello quantizzato scelto per la v1:

```sh
mkdir -p models
curl -fL \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin \
  -o models/ggml-small-q5_1.bin
```

Il file atteso occupa 190.085.487 byte e ha SHA-256
`ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb`.
`start` resta in foreground, normalizza il WAV in PCM mono 16 kHz e salva audio,
metadati e transcript in una nuova directory di sessione:

```sh
cargo run --release -- start \
  --input-wav benchmarks/fixtures/italian_synthetic.wav \
  --model models/ggml-small-q5_1.bin
```

Il simulatore carica il WAV in memoria ed è limitato esplicitamente a 64 MiB e
30 minuti. La cattura live non usa questo buffer completo e non ha tale limite.

Da un secondo terminale, usando la stessa `--sessions-dir` se diversa dal valore
predefinito `sessions`, si può controllare la sessione:

```sh
cargo run -- status
cargo run -- pause
cargo run -- resume
cargo run -- stop
```

`status` restituisce un report JSON con metadati, durata, stato, backlog, ritardo
rispetto all'ultimo segmento ASR e percorsi dei file della sessione.

La pausa riguarda solo la segmentazione e la futura trascrizione: l'audio continua
a essere scritto. Il segmento pendente viene chiuso quando inizia la pausa e
l'elaborazione riparte senza attraversare l'intervallo sospeso.

Il VAD usa chunk mono 16 kHz, soglia RMS, isteresi di avvio, preroll/postroll,
silenzio finale e durata massima. Le soglie sono configurabili tramite le opzioni
`--vad-*` mostrate da `start --help`; i valori effettivi sono salvati in
`session.json`. Coda e metriche di segmentazione sono visibili in `status.json`.

Il worker ASR usa una coda bounded separata e scrive ogni segmento finale,
senza correzioni successive, in `transcript.jsonl`. Identità del modello,
configurazione d'inferenza, backlog e real-time factor sono persistiti nella
sessione.

## Diagnostica ed export

`devices` elenca in JSON i dispositivi CoreAudio visibili, le direzioni supportate
e le configurazioni predefinite:

```sh
cargo run -- devices
```

`doctor` verifica Apple Silicon, directory delle sessioni, spazio libero, modello,
input audio e presenza di BlackHole senza aprire stream o modificare macOS:

```sh
cargo run -- doctor
cargo run -- doctor --model /percorso/al/modello.bin
```

L'assenza di un prerequisito produce `ready: false` ed exit code non zero. Senza
`--probe-audio`, permesso microfono e routing effettivo restano esplicitamente non
verificati.

Ogni export deriva da `transcript.jsonl` e conserva il testo ASR senza correzioni:

```sh
cargo run -- export sessions/<sessione> --format text
cargo run -- export sessions/<sessione> --format markdown --output transcript.md
cargo run -- export sessions/<sessione> --format srt --output transcript.srt
cargo run -- export sessions/<sessione> --format vtt --output transcript.vtt
```

Sono supportati `jsonl`, `text`, `markdown`, `srt` e `vtt`. Senza `--output`,
l'export viene scritto sullo standard output. SIGINT e SIGTERM finalizzano i dati
parziali, marcano la sessione come fallita e ne riportano il percorso recuperabile.
L'export rifiuta come destinazione qualsiasi file canonico della sessione.

Il benchmark riproducibile, le fixture e i risultati di riferimento per Apple M5
sono descritti in [`benchmarks/README.md`](benchmarks/README.md).

## Documentazione

- [Contesto completo e scelte architetturali](PROJECT_CONTEXT.md)
- [Roadmap di implementazione](ROADMAP.md)
- [Setup BlackHole su macOS](docs/BLACKHOLE_MACOS.md)
- [Storico delle modifiche](CHANGELOG.md)

## Sviluppo locale

Il progetto richiede Rust 1.88 o successivo. I controlli della milestone corrente
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
