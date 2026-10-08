# personal-transcriber

CLI locale per registrare e trascrivere riunioni in corso. La prima versione è
destinata a macOS Apple Silicon, usa BlackHole per leggere l'audio della call e
conserva audio e trascrizioni esclusivamente sul computer dell'utente.

Il progetto è in sviluppo iniziale. Può acquisire microfono e BlackHole su stream
separati oppure simulare una sessione da WAV, segmentare localmente il parlato e
trascriverlo in italiano o inglese con whisper.cpp e accelerazione Metal. La
cattura live richiede ancora la verifica end-to-end su una call reale prima di
considerare completata la milestone.

## Principi

- Nessuna API cloud, telemetria o upload di audio.
- Trascrizione ASR non rielaborata da LLM, correttori o riassuntori.
- Registrazione audio progressiva e transcript con timestamp.
- CLI prima di qualsiasi interfaccia grafica.
- Core Rust portabile; adattatore di cattura macOS nella prima versione.

## Installazione

Servono macOS su Apple Silicon, Rust 1.88 o successivo e BlackHole. Installare e
configurare BlackHole seguendo la [guida macOS](docs/BLACKHOLE_MACOS.md), quindi
compilare:

```sh
git clone https://github.com/FabioTognaa/personal-transcriber.git
cd personal-transcriber
cargo build --release
```

Scaricare il modello quantizzato predefinito:

```sh
mkdir -p models
curl -fL \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin \
  -o models/ggml-small-q5_1.bin
```

Il file atteso occupa 190.085.487 byte e ha SHA-256
`ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb`.
Il percorso `models/ggml-small-q5_1.bin` è usato automaticamente da `doctor` e
`start`.

### Scelta del modello

`--model-preset` seleziona un modello documentato senza scrivere percorsi. Il
valore predefinito è `small`; `large-v3-turbo` è il compromesso consigliato tra
accuratezza e velocità, `large-v3` la qualità massima. Un file GGML qualsiasi si
seleziona con `--model <percorso>`, che ha la precedenza sul preset.

Se avvii `start` da un terminale **senza** `--model` né `--model-preset`, compare
un menu che elenca i preset con lo stato `[scaricato]`/`[mancante]` e gli altri
file `.bin` presenti in `models/`:

```text
Seleziona il modello per la trascrizione:
  1) small            [scaricato] models/ggml-small-q5_1.bin
  2) medium           [mancante]  models/ggml-medium-q5_0.bin
  3) large-v3-turbo   [scaricato] models/ggml-large-v3-turbo-q8_0.bin
  4) large-v3         [mancante]  models/ggml-large-v3-q5_0.bin
Invio = default (small), numero = scegli, p = percorso personalizzato:
```

Invio accetta il default `small`, un numero sceglie la voce, `p` permette di
digitare un percorso. In script, CI o test (nessun terminale) il menu non compare
e vale il default `small`; passando `--model` o `--model-preset` il menu è saltato.

| Preset | File | Dimensione | Uso consigliato |
| --- | --- | ---: | --- |
| `small` | `ggml-small-q5_1.bin` | ~190 MB | predefinito, molto veloce |
| `medium` | `ggml-medium-q5_0.bin` | ~540 MB | via di mezzo |
| `large-v3-turbo` | `ggml-large-v3-turbo-q8_0.bin` | ~874 MB | migliore qualità/velocità |
| `large-v3` | `ggml-large-v3-q5_0.bin` | ~1,1 GB | qualità massima, più lento |

Per usare `large-v3-turbo`:

```sh
curl -fL \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q8_0.bin \
  -o models/ggml-large-v3-turbo-q8_0.bin
cargo run --release -- start --model-preset large-v3-turbo --language it
```

Su Apple Silicon con almeno 16 GiB, `large-v3-turbo` e `large-v3` restano sotto
il real-time factor 1 con margine ampio; le versioni F16 (`ggml-large-v3-turbo.bin`,
`ggml-large-v3.bin`) sono selezionabili con `--model` per la massima accuratezza.

### Prompt iniziale

Whisper accetta un prompt iniziale che orienta punteggiatura, maiuscole e
vocabolario senza alcuna post-elaborazione del testo. `start` usa
automaticamente un prompt predefinito per `--language` (`it` o `en`).
`--asr-prompt "testo"` lo sostituisce, ad esempio per aggiungere un glossario di
nomi, sigle e termini ricorrenti; `--asr-prompt ""` lo disattiva. Il prompt
effettivo è salvato in `session.json` e in ogni riga di `transcript.jsonl`.

## Cattura live

Verificare i nomi esatti dei dispositivi e i prerequisiti:

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
  --microphone "Microfono MacBook Air" \
  --system-audio "BlackHole 2ch" \
  --language it
```

`--language` accetta `it` oppure `en` e usa `it` come valore predefinito.

Ogni sessione è una cartella `sessions/YYYY-MM-DD_HH-MM-SS/` (ora locale di
inizio). Il contenuto è descritto in [`sessions/README.md`](sessions/README.md).
La sessione conserva `audio/microphone.wav`, `audio/system.wav` e
`audio/mixed.wav`. I callback CoreAudio copiano soltanto i campioni in una coda
bounded: resampling, mix, disco, VAD e ASR avvengono fuori dal callback. Overflow,
disconnessione, cambio di sample rate o oltre cinque secondi di audio non abbinato
arrestano la sessione come fallita invece di perdere dati senza segnalarlo.
`status.json` espone picchi, presenza segnale, drift e blocchi callback persi.

## Simulatore WAV

`start` resta in foreground, normalizza il WAV in PCM mono 16 kHz e salva audio,
metadati e transcript in una nuova directory di sessione:

```sh
cargo run --release -- start \
  --input-wav benchmarks/fixtures/italian_synthetic.wav \
  --language it
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

La pausa riguarda solo la segmentazione e la trascrizione: l'audio continua a
essere scritto. Il segmento pendente viene chiuso quando inizia la pausa e
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
input audio e presenza di BlackHole senza modificare macOS:

```sh
cargo run -- doctor
cargo run -- doctor --model /percorso/al/modello.bin
```

L'assenza di un prerequisito produce `ready: false` ed exit code non zero. Senza
`--probe-audio`, permesso microfono e routing effettivo restano esplicitamente non
verificati e il report contiene un warning.

Ogni export deriva da `transcript.jsonl` e conserva il testo ASR senza correzioni:

```sh
cargo run -- export sessions/<sessione> --format text
cargo run -- export sessions/<sessione> --format markdown --output transcript.md
cargo run -- export sessions/<sessione> --format srt --output transcript.srt
cargo run -- export sessions/<sessione> --format vtt --output transcript.vtt
```

Sono supportati `jsonl`, `text`, `markdown`, `srt` e `vtt`. Senza `--format` viene
prodotto Markdown; senza `--output`, l'export viene scritto sullo standard output.
SIGINT e SIGTERM finalizzano i dati parziali, marcano la sessione come fallita e
ne riportano il percorso recuperabile. L'export rifiuta come destinazione qualsiasi
file canonico della sessione. Per esportare una sessione fallita o ancora attiva
occorre confermare esplicitamente con `--allow-partial`.

Il benchmark riproducibile, le fixture e i risultati di riferimento per Apple M5
sono descritti in [`benchmarks/README.md`](benchmarks/README.md).

## Limiti noti

- La v1 supporta macOS su Apple Silicon; Windows e Linux non sono supportati.
- La lingua deve essere scelta esplicitamente tra italiano e inglese; non esiste
  rilevamento automatico.
- Non sono incluse diarizzazione, identificazione degli speaker, UI o funzioni cloud.
- I file delle sessioni non sono cifrati a riposo.
- Il rilevamento del parlato usa una soglia energetica RMS, non un modello VAD.
- La cattura live non è ancora stata validata end-to-end su una call reale.

## Documentazione

- [Funzionamento e architettura](docs/RELAZIONE.md)
- [Contesto completo e scelte architetturali](PROJECT_CONTEXT.md)
- [Roadmap di implementazione](ROADMAP.md)
- [Setup BlackHole su macOS](docs/BLACKHOLE_MACOS.md)
- [Come leggere una cartella di sessione](sessions/README.md)
- [Storico delle modifiche](CHANGELOG.md)

## Sviluppo locale

I controlli della milestone corrente sono:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

Il test con un modello Whisper reale è ignorato per impostazione predefinita e
richiede `PERSONAL_TRANSCRIBER_MODEL`:

```sh
PERSONAL_TRANSCRIBER_MODEL=models/ggml-small-q5_1.bin \
  cargo test real_whisper_model_processes_local_audio -- --ignored
```

## Privacy e consenso

Prima di registrare o trascrivere una riunione, informa i partecipanti e verifica le
politiche aziendali e gli obblighi applicabili. Il fatto che l'elaborazione sia locale
non elimina questo requisito.

## Licenza

MIT. Vedi [LICENSE](LICENSE).
