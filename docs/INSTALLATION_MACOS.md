# Installazione per macOS Apple Silicon

## Requisiti

- macOS su Apple Silicon
- Rust 1.88+
- BlackHole.

Installare e configurare BlackHole (se ancora non lo si possiede sulla machcina) seguendo la [guida macOS](BLACKHOLE_MACOS.md), quindi
compilare:

## 1. Clonazione e configurazione dell'ambiente

```sh
git clone https://github.com/FabioTognaa/personal-transcriber.git
cd personal-transcriber
cargo build --release
```

## 2. Modelli

ATTENZIONE: i modelli quantizzati sono molteplici e non sono inclusi nel repo di
default. È possibile scaricarli tramite i comandi elencati qui sotto.

Dalla root del progetto, scaricare il modello predefinito:

```sh
mkdir -p models
curl -fL \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin \
  -o models/ggml-small-q5_1.bin
```

Per scaricare l'altro modello presente, `large-v3-turbo`:

```sh
curl -fL \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q8_0.bin \
  -o models/ggml-large-v3-turbo-q8_0.bin
```

### Scelta del modello

Se avvii `start` da un terminale comparirà un menu che elenca i preset con lo stato `[scaricato]`/`[mancante]` e gli altri
file `.bin` presenti in `models/`; si può selezionare quello con cui si desidera procedere.

Consiglio caldamente di usare `large-v3-turbo` per la trascrizione, la qualità del prodotto finale è superiore, anche se richiede un effort più imposrtante lato processore e memoria.

`--model-preset` è il flag da accompagnare a `start` che consente di selezionare un modello specifico tra quelli documentati senza scrivere percorsi.
Il valore predefinito è `small`; come detto poco fa, consiglio di usare il modello `large-v3-turbo` per avere il miglior compromesso tra accuratezza e velocità.

Un file GGML qualsiasi siseleziona con `--model <percorso>`, che ha la precedenza sul preset.

Esempio di utilizzo:

```sh
cargo run --release -- start --model-preset large-v3-turbo --language it
```

Su Apple Silicon con almeno 16 GiB, `large-v3-turbo` e `large-v3` restano sotto
il real-time factor 1 con margine ampio; le versioni F16 (`ggml-large-v3-turbo.bin`,
`ggml-large-v3.bin`) sono selezionabili con `--model` per la massima accuratezza.
