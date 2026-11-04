# Benchmark ASR

Il benchmark esegue il motore scelto — whisper.cpp oppure ONNX Runtime per
Parakeet — fuori dai test funzionali e salva un report JSON con identità di
modello e input, hardware, configurazione, tempi, real-time factor (RTF), picco
RSS e, quando esiste un riferimento, WER e CER.

## Modelli

Non esiste un modello predefinito. Il benchmark accetta qualsiasi artefatto: un
file GGML per Whisper oppure una directory bundle per Parakeet.

Modelli Whisper documentati in `src/model.rs`:

| Preset | File | Byte | SHA-256 (inizio) |
| --- | --- | ---: | --- |
| `small` | `ggml-small-q5_1.bin` | 190.085.487 | `ae85e4a9…` |
| `medium` | `ggml-medium-q5_0.bin` | 539.212.467 | `19fea4b3…` |
| `large-v3-turbo` | `ggml-large-v3-turbo-q8_0.bin` | 874.188.075 | `317eb69c…` |
| `large-v3` | `ggml-large-v3-q5_0.bin` | 1.081.140.203 | `d75795ec…` |

Parakeet TDT 0.6B v3, variante int8 (CC-BY-4.0), documentata come bundle:

| File | Byte | SHA-256 (inizio) |
| --- | ---: | --- |
| `config.json` | 97 | `666903c7…` |
| `nemo128.onnx` | 139.764 | `a9fde148…` |
| `encoder-model.int8.onnx` | 652.183.999 | `6139d2fa…` |
| `decoder_joint-model.int8.onnx` | 18.202.004 | `eea7483e…` |
| `vocab.txt` | 93.939 | `d5854467…` |

Per scaricare solo la variante int8:

```sh
huggingface-cli download istupakov/parakeet-tdt-0.6b-v3-onnx \
  --include config.json vocab.txt nemo128.onnx encoder-model.int8.onnx decoder_joint-model.int8.onnx \
  --local-dir models/parakeet-tdt-0.6b-v3-onnx
```

Parakeet è incluso nella build standard.

## Esecuzione

```sh
cargo run --release --bin asr-benchmark -- \
  --model models/parakeet-tdt-0.6b-v3-onnx \
  --input benchmarks/fixtures/italian_synthetic.wav \
  --reference benchmarks/fixtures/italian_synthetic.txt \
  --output benchmarks/results/apple-m5-parakeet-v3-int8-synthetic.json
```

Il default esegue un warmup e cinque iterazioni misurate. Whisper usa Metal,
flash attention, beam search 5 e al massimo otto thread; Parakeet usa ONNX
Runtime su CPU, senza arena allocator, con chunk massimi di 15 s. `--language`
sceglie la lingua e `--prompt` imposta il prompt iniziale, così si può misurare
anche l'effetto del prompt. Il report registra i valori effettivi. WER e CER
usano una normalizzazione minuscola senza punteggiatura; non sono metriche del
transcript canonico, che conserva invece l'output ASR verbatim.

## Risultato di riferimento

Misura del 6 ottobre 2026 su Apple M5 con 32 GiB, macOS 27.0.1,
`whisper-rs` 0.16.0 / whisper.cpp 1.8.3:

| Fixture | Durata | RTF mediano | RTF p95 | WER | CER |
| --- | ---: | ---: | ---: | ---: | ---: |
| `italian_synthetic.wav` (small Q5_1) | 12,46 s | 0,072 | 0,078 | 5,88% | 0,47% |
| `italian_public.wav` (small Q5_1) | 1,65 s | 0,245 | 0,253 | 0% | 0% |

## Confronto Whisper / Parakeet

Misura del 3 novembre 2026 su Apple M5 con 32 GiB, fixture
`italian_synthetic.wav` (12,46 s), senza prompt:

| Modello | Motore | RTF mediano | RTF p95 | WER | CER | Picco RSS | Caricamento |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `large-v3-turbo-q8_0` | whisper.cpp / Metal | 0,187 | 0,192 | 0% | 0% | 1,12 GiB | 11,0 s¹ |
| `parakeet-v3-int8` | ONNX Runtime / CPU | 0,032 | 0,032 | 0% | 0% | 1,06 GiB | 1,4 s |

¹ Include la compilazione della libreria Metal alla prima esecuzione; le
esecuzioni successive caricano in circa 0,7 s.

Parakeet è circa **5,8 volte più veloce** di turbo a parità di WER su questa
fixture pulita. Non ha prompt/glossario, quindi su nomi propri e sigle resta da
misurare su riunioni reali.

## Silenzio e allucinazioni

Fixture di 60 s di silenzio, riferimento vuoto. Un WER di 1,0 significa che il
modello ha prodotto testo dove non c'era parlato.

| Modello | RTF mediano | WER | Testo prodotto |
| --- | ---: | ---: | --- |
| `large-v3-turbo-q8_0` | 0,064 | 1,0 | “Autore dei sottotitoli e revisione a cura di QTSS …” (ripetuto) |
| `parakeet-v3-int8` | 0,031 | 0,0 | *(vuoto)* |

La fixture si rigenera con:

```sh
python3 - <<'PY'
import wave
with wave.open('benchmarks/fixtures/silence_60s.wav', 'wb') as w:
    w.setnchannels(1); w.setsampwidth(2); w.setframerate(16000)
    w.writeframes(b'\x00\x00' * 16000 * 60)
PY
: > benchmarks/fixtures/silence_60s.txt
```

Il file è ignorato da git: in repository restano solo i numeri.

## Limiti

Le fixture sono smoke test, non una stima della qualità su riunioni reali. Prima
di eleggere un modello servono registrazioni pubbliche più lunghe e
rappresentative, con sovrapposizioni, rumore e nomi propri.
