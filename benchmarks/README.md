# Benchmark ASR

Il benchmark esegue whisper.cpp fuori dai test funzionali e salva un report JSON
con identità di modello e input, hardware, configurazione, tempi, real-time factor
(RTF), picco RSS e, quando esiste un riferimento, WER e CER.

## Modello v1

La scelta iniziale è
[`ggml-small-q5_1.bin`](https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin):

- dimensione: 190.085.487 byte;
- SHA-256: `ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb`;
- quantizzazione: Q5_1;
- lingue: modello multilingue, usato esplicitamente con `it`.

È abbastanza piccolo per la distribuzione locale ma, sulla fixture sintetica,
mantiene un margine ampio rispetto al requisito RTF < 1.

## Esecuzione

```sh
cargo run --release --bin asr-benchmark -- \
  --model models/ggml-small-q5_1.bin \
  --input benchmarks/fixtures/italian_synthetic.wav \
  --reference benchmarks/fixtures/italian_synthetic.txt \
  --output benchmarks/results/apple-m5-small-q5_1-synthetic.json
```

Il default esegue un warmup e cinque iterazioni misurate, con Metal, flash
attention, beam search 5 e al massimo otto thread. Il report registra i valori
effettivi. WER e CER usano una normalizzazione minuscola senza punteggiatura; non
sono metriche del transcript canonico, che conserva invece l'output ASR verbatim.

## Risultato di riferimento

Misura del 6 ottobre 2026 su Apple M5 con 32 GiB, macOS 27.0.1,
`whisper-rs` 0.16.0 / whisper.cpp 1.8.3:

| Fixture | Durata | RTF mediano | RTF p95 | WER | CER |
| --- | ---: | ---: | ---: | ---: | ---: |
| `italian_synthetic.wav` | 12,46 s | 0,072 | 0,078 | 5,88% | 0,47% |
| `italian_public.wav` | 1,65 s | 0,245 | 0,253 | 0% | 0% |

Il caricamento iniziale del modello ha richiesto 10,16 s e il picco RSS osservato
è stato circa 504 MiB. Le fixture, soprattutto quella pubblica molto corta, sono
smoke test e non una stima della qualità su riunioni reali; prima di cambiare
modello servono registrazioni pubbliche più lunghe e rappresentative.
