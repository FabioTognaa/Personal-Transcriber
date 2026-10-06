# live-transcript

CLI locale per registrare e trascrivere riunioni in corso. La prima versione è
destinata a macOS Apple Silicon, usa BlackHole per leggere l'audio della call e
conserva audio e trascrizioni esclusivamente sul computer dell'utente.

Il progetto è in fase di progettazione: non esiste ancora un binario utilizzabile.

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

## Documentazione

- [Contesto completo e scelte architetturali](PROJECT_CONTEXT.md)
- [Setup BlackHole su macOS](docs/BLACKHOLE_MACOS.md)
- [Storico delle modifiche](CHANGELOG.md)

## Privacy e consenso

Prima di registrare o trascrivere una riunione, informa i partecipanti e verifica le
politiche aziendali e gli obblighi applicabili. Il fatto che l'elaborazione sia locale
non elimina questo requisito.

## Licenza

MIT. Vedi [LICENSE](LICENSE).
