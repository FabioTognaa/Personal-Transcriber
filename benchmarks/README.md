# Benchmark ASR

Il benchmark esegue whisper.cpp fuori dai test funzionali e salva un report JSON
con identità di modello e input, hardware, configurazione, tempi, real-time factor
(RTF), picco RSS e, quando esiste un riferimento, WER e CER.

## Modelli

Il modello predefinito resta
[`ggml-small-q5_1.bin`](https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin):

- dimensione: 190.085.487 byte;
- SHA-256: `ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb`;
- quantizzazione: Q5_1;
- lingue: modello multilingue, usato esplicitamente con `it`.

`--model-preset` aggiunge modelli più accurati e più pesanti, verificati da
`doctor` (dimensione e SHA-256 documentati in `src/model.rs`):

| Preset | File | Byte | SHA-256 (inizio) |
| --- | --- | ---: | --- |
| `small` | `ggml-small-q5_1.bin` | 190.085.487 | `ae85e4a9…` |
| `medium` | `ggml-medium-q5_0.bin` | 539.212.467 | `19fea4b3…` |
| `large-v3-turbo` | `ggml-large-v3-turbo-q8_0.bin` | 874.188.075 | `317eb69c…` |
| `large-v3` | `ggml-large-v3-q5_0.bin` | 1.081.140.203 | `d75795ec…` |

Qualsiasi altro GGML si seleziona con `--model <percorso>`.

## Esecuzione

```sh
cargo run --release --bin asr-benchmark -- \
  --model models/ggml-small-q5_1.bin \
  --input benchmarks/fixtures/italian_synthetic.wav \
  --reference benchmarks/fixtures/italian_synthetic.txt \
  --output benchmarks/results/apple-m5-small-q5_1-synthetic.json
```

Il default esegue un warmup e cinque iterazioni misurate, con Metal, flash
attention, beam search 5 e al massimo otto thread. `--language` sceglie la lingua
e `--prompt` imposta il prompt iniziale, così si può misurare anche l'effetto del
prompt. Il report registra i valori effettivi. WER e CER usano una normalizzazione
minuscola senza punteggiatura; non sono metriche del transcript canonico, che
conserva invece l'output ASR verbatim.

## Risultato di riferimento

Misura del 6 ottobre 2026 su Apple M5 con 32 GiB, macOS 27.0.1,
`whisper-rs` 0.16.0 / whisper.cpp 1.8.3:

| Fixture | Durata | RTF mediano | RTF p95 | WER | CER |
| --- | ---: | ---: | ---: | ---: | ---: |
| `italian_synthetic.wav` (small Q5_1) | 12,46 s | 0,072 | 0,078 | 5,88% | 0,47% |
| `italian_public.wav` (small Q5_1) | 1,65 s | 0,245 | 0,253 | 0% | 0% |

## Confronto modelli

Misura dell'8 ottobre 2026 sulla stessa macchina e fixture
`italian_synthetic.wav`, senza prompt:

| Modello | RTF mediano | RTF p95 | WER | CER | Picco RSS | Caricamento |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `small-q5_1` | 0,074 | 0,077 | 5,88% | 0,47% | ~488 MiB | 0,36 s |
| `large-v3-turbo-q8_0` | 0,182 | 0,186 | 0% | 0% | ~1,1 GiB | 0,66 s |

`large-v3-turbo` resta circa 5,5 volte più veloce del tempo reale e azzera gli
errori della fixture, con punteggiatura e maiuscole corrette laddove `small`
sbagliava (ad esempio univa `invia dati` in `inviadati` e usava una virgola al
posto del punto). Il prompt predefinito italiano non ha cambiato questo output
già corretto, ma serve su audio rumoroso e su nomi propri: va misurato con
registrazioni rappresentative, non con questa fixture sintetica molto pulita.

Le fixture, soprattutto quella pubblica molto corta, sono smoke test e non una
stima della qualità su riunioni reali; prima di cambiare il modello predefinito
servono registrazioni pubbliche più lunghe e rappresentative.
