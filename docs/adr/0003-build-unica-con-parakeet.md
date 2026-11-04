# Una sola build ufficiale con Parakeet

La feature `parakeet` è attiva di default: `cargo build --release` e `cargo
install` producono un unico binario con Whisper e Parakeet. Tenere due build
installate in parallelo generava confusione: un binario vecchio in PATH e un
modello che spariva dal menu. L'uscita di sicurezza resta `--no-default-features
--features metal`, per compilare senza ONNX Runtime su piattaforme o build che non
lo supportano.

## Conseguenze

- Il binario standard richiede ONNX Runtime e, su macOS, il link a
  `libclang_rt.osx`.
- Documentazione e CI usano un solo comando, senza `--features parakeet`.
- La build “solo Whisper” non è un prodotto supportato: è una scappatoia tecnica.
