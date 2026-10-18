# USB: xHCI e teclado/mouse HID

Plano do primeiro driver USB do Kitsune. **Estado: E0 e E1 feitas (descoberta do controlador e núcleo puro, testados no host); E2 a E5 por fazer. Ainda não existe driver.** Hoje a entrada é
só PS/2 (`kernel/src/ps2.rs`), e `docs/HARDWARE.md` e `docs/BOOT-USB.md` já registram que notebooks modernos
podem não ter PS/2 nem emulação dele. Sem teclado e mouse o resto do roadmap não chega a uma máquina real.

## Escopo

**Entra:** controlador xHCI (o único em PCs atuais), teclado e mouse HID pelo *boot protocol*, portas ligadas
direto ao controlador, tudo por polling.

**Não entra (decisão reversível, fica para depois):** hub USB, hot-plug depois do boot, parser do *report
descriptor* (touchpad, mouse com botões extras ou alta resolução), EHCI/OHCI/UHCI, armazenamento USB,
touchpad I2C-HID dos notebooks, Wi-Fi.

Por que boot protocol primeiro: teclado e mouse o entendem sem ler o descriptor, o relatório tem formato fixo
(8 bytes de teclado, 3–4 de mouse) e dá para testar tudo no host.

## O que o código de hoje impõe

| Fato (lido no código) | Consequência |
|---|---|
| `kernel/src/pci.rs` só varre o **barramento 0** | xHCI em PC real costuma ficar atrás de uma ponte. Varrer todos os barramentos e ler classe/prog-if (`0x0C0330`) é a primeira etapa. |
| Sem APIC nem MSI; IRQs pelo PIC | O driver usa **polling** do *event ring* no laço do compositor, como o AC'97 e o NE2000. Latência de entrada = período do laço. |
| `ac97.rs` guarda DMA em memória estática abaixo de 4 GiB e converte com `virtio::virt_to_phys` | Mesmo modelo para DCBAA, anéis, contextos e *scratchpad*. |
| `main.rs` consome `ps2::poll()` → `Event::{Mouse,Key}` | O USB produz os **mesmos** `Event`: HID vira scancode set-1 (+ `extended`) e `Packet`. O desktop não muda. |
| `bar_base`/`cap_addr` recusam BAR fora do que o bootloader mapeou | Confirmar que o BAR do xHCI no QEMU e em PC real está dentro do mapa; senão mapear explicitamente. |

## Etapas

Lógica pura vai para `kitsune_core` com teste que falha antes e passa depois (`unsafe` proibido ali); o
kernel só tem a cola de hardware. Esforço em dias de um desenvolvedor: **estimativa minha**, não medida.

| # | Etapa | Onde | Aceite | Dias |
|---|---|---|---|---|
| E0 (feita) | Varrer todos os barramentos; classe e prog-if; tamanho e 64 bits do BAR | `core/hw/pci`, `kernel/pci.rs` | testes de host da enumeração; o log do guest lista o xHCI do QEMU | 2 |
| E1 (feita) | Núcleo xHCI puro: registradores, TRB (codificar/decodificar), anel com *cycle bit* e Link TRB, contextos de slot e endpoint, velocidade → tamanho máx. de pacote | `core/hw/xhci` | testes, inclusive anel dando a volta e entrada hostil | 4 |
| E2 | Descritores USB (device/config/interface/endpoint), detecção de interface HID boot, relatório de teclado → eventos de tecla (diferença entre relatórios, tabela HID → set-1), relatório de mouse → `Packet` | `core/hw/usb`, `core/hw/hid` | testes; alvo de fuzz para os parsers (o projeto já tem 17) | 3 |
| E3 | Driver: tomar o controlador do firmware (USBLEGSUP), reset, DCBAA, anel de comandos e de eventos, *scratchpad*, reset de porta, Enable Slot, Address Device, transferência de controle (GET_DESCRIPTOR, SET_CONFIGURATION, SET_PROTOCOL boot, SET_IDLE), Configure Endpoint e leitura do endpoint de interrupção por polling | `kernel/xhci.rs` | no QEMU, log `usb: teclado` e `usb: mouse`, e relatórios chegando | 8–12 |
| E4 | Ligar ao laço do `main.rs` junto de `ps2::poll()`; sem PS/2 o sistema segue | `kernel/main.rs`, `kernel/usb.rs` | digitar e clicar com PS/2 desligado | 2 |
| E5 | Cenário de aceite e documentação | `tools/perf/scen/`, `CHANGELOG`, `ROADMAP`, `TESTING`, `HARDWARE` | ver abaixo | 2 |

Total: **~21 a 25 dias úteis (4 a 5 semanas)**. E3 concentra o risco.

## Aceite no QEMU

`QEMU_EXTRA="-device qemu-xhci -device usb-kbd -device usb-mouse"` mais o PS/2 desligado. O cenário (nome
tentativo `w64-usb-hid.sh`) precisa provar, pelo serial e por screenshot:

1. o log do kernel mostra o xHCI, as duas portas e os dois dispositivos HID;
2. digitar no Terminal produz o texto certo (letras, Shift, Enter, setas, F-keys);
3. mover e clicar abre um app pelo dock;
4. com PS/2 **e** USB juntos nada duplica nem trava.

## Riscos e perguntas em aberto

- **Roteamento de entrada do QEMU (verificado com o QEMU 11.1 instalado).** `-machine pc,i8042=off` existe e o
  kernel boota até o primeiro quadro sem PS/2 (o log ainda imprime `ps2: mouse id 0`, é só o timeout do
  controlador ausente). Com `-device qemu-xhci -device usb-kbd -device usb-mouse`, `info mice` lista só o `QEMU HID
  Mouse`; com o PS/2 ligado também, o monitor marca o USB como o ponteiro ativo. Para o teclado não há comando
  equivalente: confirmar com o driver pronto que o `sendkey` chega ao `usb-kbd`.
- **Entrega do mouse:** o monitor manda movimento relativo; `usb-mouse` é relativo, `usb-tablet` é absoluto.
  Começar por `usb-mouse`.
- **Handoff BIOS→SO (USBLEGSUP) e erratas de chipset não aparecem no QEMU.** Só a rodada em PC real mostra.
  Implementar o handoff conforme a especificação mesmo sem poder provar.
- **Polling:** a latência de entrada depende do laço do compositor. Medir com o `[trace]` do guest; se ficar
  ruim, a saída é MSI-X, que depende do APIC (item 12 do roadmap).
- **Memória DMA:** definir o tamanho da região estática (anéis + contextos + *scratchpad* por dispositivo) e
  garantir alinhamento e que fique abaixo de 4 GiB.

## Depois do MVP

Hub USB, hot-plug, parser de *report descriptor* (touchpad, botões extras), MSI-X, armazenamento em massa USB.
