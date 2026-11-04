# Scribroom

Scribroom registra e trascrive riunioni in locale: l'audio non esce dalla macchina e ogni elaborato resta una sessione sul disco.

## Language

**Motore (engine)**:
Il runtime che esegue un modello e trasforma l'audio in testo, per esempio whisper.cpp oppure ONNX Runtime.
_Avoid_: modello, backend

**Modello (model)**:
Un insieme specifico di pesi con un nome riconoscibile, per esempio `large-v3-turbo` o `parakeet-0.6b-v3`.
_Avoid_: motore, preset

**Artefatto (artifact)**:
La forma di un modello su disco: un singolo file oppure una directory con più file.
_Avoid_: file del modello

**Preset**:
Un nome stabile da riga di comando che punta a un artefatto documentato.
_Avoid_: default, modello predefinito

**Scelta del modello (model selection)**:
L'unica scelta esposta all'utente: quale modello usare. Il motore si deduce dall'artefatto e non si sceglie.
_Avoid_: scelta del motore, scelta del backend

**Disponibile (available)**:
Un artefatto presente su disco. Solo gli artefatti disponibili compaiono nel menu.
_Avoid_: installato, scaricato
