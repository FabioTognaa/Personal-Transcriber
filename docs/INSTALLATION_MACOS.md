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
cargo install --path . --bin scrib
```

Questo installa un unico binario con Whisper e Parakeet. Per aggiornare una copia
già installata dopo una modifica al codice, aggiungi `--force`.

Il comando installato si chiama `scrib`.

## 2. Modelli

I modelli non sono inclusi nel repository e vanno messi in `models/`. Non esiste un
modello predefinito: `start`, `transcribe` e `doctor` usano quello che scegli. La
cartella si può cambiare con `SCRIB_MODELS_DIR`.

### Whisper

Dalla root del progetto, scarica almeno un file GGML. Consigliato `large-v3-turbo`
per il miglior compromesso qualità/velocità:

```sh
mkdir -p models
curl -fL \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q8_0.bin \
  -o models/ggml-large-v3-turbo-q8_0.bin
```

Gli altri preset documentati:

| Preset           | File                           | Dimensione | Uso consigliato            |
| ---------------- | ------------------------------ | ---------: | -------------------------- |
| `medium`         | `ggml-medium-q5_0.bin`         |    ~540 MB | via di mezzo               |
| `large-v3-turbo` | `ggml-large-v3-turbo-q8_0.bin` |    ~874 MB | migliore qualità/velocità  |
| `large-v3`       | `ggml-large-v3-q5_0.bin`       |    ~1,1 GB | qualità massima, più lento |

Un file GGML qualsiasi si seleziona con `--model <percorso>`.

### Parakeet (opzionale)

Parakeet è una **directory** con più file ONNX, non un singolo `.bin`. È incluso
nella build standard. La variante int8 pesa circa 670 MB ed è distribuita con
licenza CC-BY-4.0.

```sh
mkdir -p models/parakeet-tdt-0.6b-v3-onnx
for f in config.json vocab.txt nemo128.onnx \
         encoder-model.int8.onnx decoder_joint-model.int8.onnx; do
  curl -fL \
    "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/$f" \
    -o "models/parakeet-tdt-0.6b-v3-onnx/$f"
done
```

### Scegliere il modello

Se avvii `start` o `transcribe` da un terminale, compare un menu con i **soli
modelli presenti**, ordinati per dimensione. Se ce n'è uno solo viene scelto senza
domanda; se non ce n'è nessuno il comando spiega come scaricarli. `Esc` o
`Ctrl-C` annulla.
Senza terminale e senza `--model`/`--model-preset` il comando fallisce con un
errore esplicito.

`--model-preset` seleziona un preset documentato senza scrivere percorsi; `--model`
accetta un file GGML oppure una directory Parakeet e ha la precedenza sul preset.
Esempio:

```sh
scrib start --model-preset large-v3-turbo --language it
scrib transcribe riunione.mp3 --model models/parakeet-tdt-0.6b-v3-onnx
```

Su Apple Silicon con almeno 16 GiB, `large-v3-turbo` e `large-v3` restano sotto
il real-time factor 1 con margine ampio; le versioni F16 (`ggml-large-v3-turbo.bin`,
`ggml-large-v3.bin`) sono selezionabili con `--model` per la massima accuratezza.
Parakeet int8, sullo stesso hardware, è risultato circa sei volte più veloce di
`large-v3-turbo` ed è più prudente sui silenzi; i numeri sono in
`benchmarks/README.md`.
