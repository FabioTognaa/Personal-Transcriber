# Setup BlackHole su macOS

BlackHole è un driver audio virtuale open source. Consente a un'applicazione di
produrre audio verso un dispositivo virtuale e a `live-transcript` di leggerlo come
input. Non intercetta Zoom, Google Meet o Microsoft Teams: instrada soltanto audio
locale sul Mac.

## Obiettivo

L'audio dei partecipanti deve raggiungere contemporaneamente:

1. cuffie o altoparlanti, così l'utente può ascoltare la riunione;
2. BlackHole, così il tool può registrarlo.

Il microfono è un input separato. Il tool miscela localmente microfono e BlackHole
per produrre la traccia di trascrizione v1.

## Configurazione prevista

1. Installare BlackHole 2ch seguendo le istruzioni del progetto ufficiale.
2. Aprire **Configurazione MIDI Audio** su macOS.
3. Creare un dispositivo **Multi-output**.
4. Selezionare BlackHole 2ch e l'uscita desiderata, ad esempio cuffie USB o
   Built-in Output.
5. Impostare il dispositivo Multi-output come output della call o come output di
   sistema prima di entrare nella riunione.
6. Lasciare il normale microfono selezionato come input della call.
7. In `live-transcript`, selezionare BlackHole come sorgente audio remoto e il
   microfono come sorgente locale.

Usare cuffie è preferibile: riduce eco, rientro del parlato nel microfono e feedback.

## Verifica

Prima di una riunione importante:

1. eseguire `live-transcript devices` e verificare che BlackHole e microfono siano
   elencati;
2. eseguire `live-transcript doctor`;
3. riprodurre un breve audio di prova;
4. avviare una breve sessione;
5. controllare che siano presenti segnale remoto, segnale microfono e file audio.

Il tool deve mostrare livelli/rilevamento segnale durante i controlli, ma non deve
modificare automaticamente le impostazioni audio di macOS.

## Ripristino

Al termine, selezionare nuovamente cuffie/altoparlanti normali come output di sistema
o della piattaforma di call. Se l'audio sembra sparito, quasi sempre l'output è ancora
instradato solo a BlackHole.

## Limiti

Il dispositivo Multi-output può avere controlli volume differenti dal dispositivo
normale. BlackHole non risolve ritardi, eco, audio disattivato nella call o permessi
mancanti del microfono. `live-transcript` deve segnalare l'assenza di segnale, ma non
può garantire che l'utente abbia configurato correttamente ogni applicazione.
