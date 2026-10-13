# Som

O sistema toca som por um **driver AC'97** no kernel, um **mixer** com vários fluxos e reamostragem, e chamadas de
sistema **60–65** para os processos de anel 3. É a etapa 5 do item 11 do [ROADMAP](../ROADMAP.md) (uma das três peças
para o YouTube tocar vídeo com áudio; as outras são a rede para o JavaScript e os decodificadores VP9/Opus).

| Peça | Onde | O quê |
|---|---|---|
| Registradores e contabilidade do AC'97 | `kitsune_core::hw::ac97` | mapa NAM/NABM, descritores, `PlayRing` (CIV/PICB/SR viram quadros tocados) |
| Mixer | `kitsune_core::hw::audio` | `SampleRing` (fila do fluxo), `Stream` (reamostragem, volume, pausa, relógio), `Mixer` (limites, soma, volume mestre, mudo), `Tone` (seno inteiro) |
| WAV | `kitsune_core::format::wav` | parser e cabeçalho |
| Driver | `kernel/src/ac97.rs` | PCI, reset, codec, DMA, preenchimento dos blocos |
| Serviço | `kernel/src/audio.rs` | estado atrás de um `SpinLock`, thread `audiod`, tons do kernel (`beep`), volume do sistema |
| Chamadas | `kernel/src/userproc/audio.rs` | 60–65 |
| Contrato | `user-apps/abi/src/audio.rs` (`kitsune-userabi`) | números, `AudioStatus`, operações de `audio_ctl`, limites |
| SDK | `user-apps/sdk/src/audio.rs` | `kitsune_user::audio::Stream` |
| Programa | `user-apps/kterm/src/bin/play.rs` | `play`, e os comandos `play`, `tone`, `beep`, `volume` do shell |
| Ajustes | `kernel/src/desktop/apps/ajustes/pages/sound.rs` | seção **Som**: volume, mudo, testar |
| Prova | `tools/perf/scen/w56-audio.sh`, `tools/perf/audio-check.py` | o QEMU grava o que o guest toca e o host mede |

## Por que AC'97 (e não Intel HDA)

* **O QEMU emula o AC'97 em todo host**, inclusive no build oficial de Windows com `-audiodev dsound`, que é como o
  dono roda o sistema (`run.ps1`, WHPX). O HDA (`-device intel-hda -device hda-duplex`) também existe lá; o virtio-sound
  só nos QEMU recentes (8.2+).
* **O caminho de saída cabe em uma página**: duas janelas de E/S (o codec, NAM, e o bus master, NABM), uma lista de 32
  descritores (endereço, tamanho, flags) e um motor de DMA. O HDA pede anéis de comandos (CORB/RIRB), descoberta do
  grafo de widgets do codec, descritores de stream e formato por stream: umas dez vezes o código, e cada passo pode
  travar de um jeito diferente em hardware real.
* **PCM 16 bits estéreo a 48 kHz** é o que o AC'97 faz por padrão (com VRA, outras taxas; não usamos: o mixer sempre
  entrega 48 kHz).
* Hardware real com AC'97 é de 2000–2008 (ICH0–ICH7); placas novas têm HDA. O HDA é o próximo driver quando houver
  hardware para provar; a interface com o mixer (`observe` + `fill_one`) já é a que ele precisaria.

## O driver

* **Detecção** por PCI (`8086:2415` do QEMU e os ICH até o ICH7, AMD 8111, nForce; lista em `hw::ac97::PCI_IDS`). Sem
  BARs de E/S (o ICH4+ em modo só-MMIO) o driver recusa com uma linha no log. Sem dispositivo: `audio: no device` e
  todas as chamadas respondem `ENODEV`; nenhuma thread a mais.
* **Inicialização**: espaço de E/S e bus master no comando PCI, saída do reset frio (`GLOB_CNT`), espera o codec
  (`GLOB_STA` bit 8, até 200 ms), reset do codec, volumes em 0 dB (mestre `0x0000`, PCM `0x0808`), VRA ligado e a taxa
  em 48 kHz quando o codec tem, reset da caixa PCM-out e a lista de descritores apontada (`BDBAR`). Interrupções
  desligadas: o driver é por *polling*.
* **Memória de DMA**: 32 blocos de 512 quadros (2 KiB, 10,7 ms) num `static` alinhado a 4 KiB (cada bloco fica numa
  página só, então é contíguo na memória física), e a lista de descritores. Endereços de 32 bits: o driver confere que a
  memória está abaixo de 4 GiB (com 256 MiB no QEMU sempre está).
* **Os descritores nunca mudam.** O driver só preenche blocos à frente do hardware e move o `LVI` (último válido) para
  o último bloco preenchido. Se o `audiod` atrasar, o hardware termina o último bloco válido e **para** (`DCH`): sai
  silêncio, nunca um pedaço antigo repetido. Ao preencher de novo, escrever o `LVI` retoma no bloco seguinte (se não
  retomar, o driver desliga e liga o `RPBM`).
* **Quanto à frente**: 12 blocos (~128 ms). O `audiod` olha a cada tick (4 ms), então um atraso de até ~120 ms da thread
  não falta som. Sem nada tocando ele não preenche, o hardware para e a thread dorme até um fluxo abrir.
* **Relógio do dispositivo**: `PlayRing::observe` lê `CIV` (bloco atual), `PICB` (amostras que faltam no bloco) e `SR`, e
  devolve os quadros tocados desde o início; é o tempo de dispositivo do mixer.

## O mixer

* Até **16 fluxos** (4 por dono: cada processo, e o kernel), cada um com uma fila de até **256 KiB** (~1,4 s de 44,1 kHz
  estéreo) alocada uma vez na abertura. Formato: **s16le**, mono ou estéreo, taxa de 8 a 96 kHz (qualquer inteiro). O
  kernel é *soft-float*: nada de `f32`; o `play` converte WAV de 8, 24 e 32 bits e float para 16 bits em anel 3.
* **Reamostragem** por interpolação linear com posição 32.32 (a 48 kHz o passo é exato e as amostras passam
  intactas). Barata e boa para voz, bipes e música de 44,1 kHz; um filtro melhor troca só `Stream::render`.
* **Soma** em 32 bits, volume do fluxo e volume mestre em Q16 (porcentagem linear de amplitude), saturação em 16 bits
  (dois fluxos altos cortam em vez de dar a volta; o mixer conta as amostras cortadas). **Mudo** zera a saída mas
  consome as filas: o relógio continua, então um player mudo não perde a sincronia.
* **Relógio "tocado"**: cada mistura registra "os quadros `d0..d1` do dispositivo levaram os quadros `c0..c1` deste
  fluxo"; `played` mapeia a posição do dispositivo de volta por esses segmentos. É o que o player de vídeo segue: conta
  o que **saiu** (não o que entrou no buffer de DMA), para em pausa e quando o fluxo fica sem dados, e continua certo
  depois de um atraso do dispositivo.
* **Underrun** de um fluxo: ficou sem dados enquanto tocava **e** depois chegaram mais (um buraco no meio). O fim
  natural de um fluxo não conta.

## A interface de anel 3 (chamadas 60–65)

| N | Chamada | Efeito |
|---|---|---|
| 60 | `audio_open(taxa, canais, formato)` | id do fluxo; `EINVAL` (taxa, canais, formato ≠ `AUDIO_S16LE`), `EMFILE` (limite), `ENODEV` (sem placa) |
| 61 | `audio_write(id, ptr, len)` | **não bloqueia**: aceita os quadros inteiros que cabem (até 64 KiB por chamada) e devolve os bytes; `EAGAIN` com a fila cheia |
| 62 | `audio_status(id, saida)` | `AudioStatus` (48 bytes): bytes na fila e livres, `played` (o relógio), `consumed`, `written`, underruns, taxa, canais, volume e estado |
| 63 | `audio_close(id, drenar)` | `0` para na hora; `1` toca o que está na fila e some sozinho |
| 64 | `audio_ctl(id, op, valor)` | volume do fluxo (0–100), pausa, descartar a fila (seek), volume mestre e mudo do sistema, taxa do dispositivo |
| 65 | `audio_wait(id, quer, timeout_ms)` | bloqueia até `quer` bytes livres na fila, ou (`quer = 0`) até tudo o que foi escrito ter tocado; 1 quando vale, 0 no prazo |

**Por que `write` não bloqueia.** O cliente que importa é o player do navegador, que roda num laço de eventos
(`jsrun --page`): ele escreve o que cabe e volta depois, sem prender o processo. Para quem quer bloquear há
`audio_wait`, que é reiniciada ao acordar como `page_recv` (o prazo fica guardado com o processo que espera, então o
reinício não o estende). `kitsune_user::audio::Stream::write_all` junta as duas.

**Direitos.** Qualquer processo pode tocar, dentro dos limites (4 fluxos, 256 KiB por fila, 16 no total). Os fluxos
são do processo que os abriu (outro processo recebe `EBADF`) e fecham quando ele termina (o `audiod` confere a cada
100 ms; o `reap` também). O volume mestre e o mudo valem para tudo e qualquer programa da sessão pode mudá-los, como as
teclas de volume de um teclado.

## Comandos e Ajustes

* `play arquivo.wav [-v VOL] [--clock]`: WAV PCM de 8/16/24/32 bits ou float, mono ou estéreo, qualquer taxa de 8 a
  96 kHz. Lê o arquivo aos pedaços de 32 KiB (o cabeçalho precisa estar no primeiro MiB).
* `tone FREQ[,FREQ...] [MS] [-v VOL]`: tons senoidais; várias frequências tocam juntas, um fluxo por frequência (um
  acorde, prova do mixer). `beep` é `tone 880 150`.
* `volume [N|mute|unmute]`: mostra ou muda o volume do sistema.
* `play --gen arq.wav FREQ MS [TAXA [CANAIS [BITS]]]` grava um WAV com um tom (para testes).
* `--clock` imprime o relógio do fluxo a cada 250 ms (`clock uptime_ms=... played_ms=...`).
* **Ajustes → Som**: volume do sistema (deslizante), mudo e "Testar" (um bipe do kernel). O volume e o mudo ficam em
  `/etc/kitsune.conf` (`volume=`, `mute=`) e valem desde o boot.

## Prova no QEMU

O QEMU grava o que o guest toca: `-audiodev wav,id=snd0,path=ARQ.wav,out.frequency=48000 -device AC97,audiodev=snd0`.
O cenário `tools/perf/scen/w56-audio.sh` toca, pelo Terminal, um tom de 440 Hz por 2 s, um WAV gerado no próprio guest
(660 Hz a 22,05 kHz mono 16 bits, reamostrado), outro WAV (550 Hz a 44,1 kHz estéreo 8 bits), um acorde (440 + 660 Hz,
dois fluxos), um tom com o volume do sistema em 25 %, um tom com o volume do fluxo em 50 %, um tom com o relógio
impresso e um bipe; `tools/perf/audio-check.py` mede cada som na gravação (FFT com janela de Hann e pico parabólico,
duração, nível relativo, pureza espectral, silêncio exato depois, canais iguais) e a inclinação do relógio contra o
`uptime`. Resultado em `$OUT/verdict.txt`; os números estão no CHANGELOG. `w56-audio-nodev.sh` prova o boot sem placa.

Observação: o QEMU aplica o volume do codec à sua maneira (PCM `0x0808`, que é 0 dB num codec real, sai a ~74 % na
gravação). As medidas de nível são relativas ao primeiro som, então não dependem disso.

## O que ainda não está

* **Não testado em hardware real nem com `dsound` no Windows** (só o backend `wav` do QEMU, aqui). O caminho é o
  mesmo dispositivo emulado, então o esperado é que funcione; atrasos do host Windows podem pedir mais blocos à frente.
* Só saída (sem microfone), só AC'97 (sem HDA nem virtio-sound), sem interrupções (polling a cada tick enquanto toca).
* Sem `f32` no kernel (o player converte), sem filtro de reamostragem melhor que o linear, volume linear (sem curva
  logarítmica no deslizante).
* O navegador tem `<audio>`/`<video>` (um fluxo do mixer por elemento, cujo relógio "tocado" é o relógio do player;
  ver [`tls-browser.md`](tls-browser.md) §14), mas não WebAudio.
