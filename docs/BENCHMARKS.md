# Números do README, com o comando que os reproduz

Cada número citado no [`README.md`](../README.md) e no [`README.en.md`](../README.en.md) aparece aqui com a fonte, o
comando e a data. Tudo foi medido em **2026-10-10**, na árvore da branch `w35-docs` (sem mudança de código que afete
desempenho em relação a `906526a`), com **QEMU 8.2.2 em TCG, sem KVM** (o ambiente de medição não tem `/dev/kvm`),
num host Xeon de 2,8 GHz com 4 vCPUs.

> **Leia assim.** TCG emula a CPU em software. Os tempos absolutos abaixo valem como **proporção** entre
> cenários, não como o tempo de uma máquina real nem de uma VM com KVM. Nada foi medido em hardware real
> nem com KVM. O que vem de `docs/TESTING.md` ou da auditoria e **não** foi repetido hoje está marcado
> *(de outro documento)*.

Preparo comum: `cargo build --release -p os` (imagem normal) e
`cargo build --release -p os --features perf-trace` (a mesma imagem com os contadores de quadro e de heap na
serial). As imagens ficam em `target/release/build/os/*/out/kitsune-{bios,uefi}.img`.

## Como comparar duas imagens (medida repetível)

O TCG sem KVM varia ~2× entre execuções com a carga do host, então duas imagens comparadas assim dão ruído.
Com `QEMU_EXTRA="-cpu max -icount shift=0"` o tempo virtual conta instruções (1 ns cada) e o resultado repete;
os números "virtuais" desta seção de otimização são assim (a vinheta de abertura usa o RTC, que não segue o
tempo virtual: compare só as etapas medidas em TSC). Use o build `perf-trace` e `tools/perf/summ.py`; para a
variante dos apps no kernel, `KITSUNE_APPS_LOCAL=1`. Capturas de tela dependem da hora (tema automático): fixe
`-rtc base=...` ao comparar pixels.

## Boot

Marcos `[trace] boot +N ms` que o kernel escreve na serial em qualquer build; `tools/perf/boot.py` tira a mediana.

| Cenário | Comando | Primeiro quadro do desktop, após a entrada do kernel |
|---|---|---|
| BIOS, disco novo (formata o OJFS), 3 execuções | `tools/qemu-headless.sh bios <saida> 20`, depois `python3 tools/perf/boot.py <saida>...` | mediana **8,78 s** (mín. 8,28, máx. 8,88) |
| BIOS, disco já formatado (`KEEP_FS=1`), 3 execuções | `KEEP_FS=1 tools/qemu-headless.sh bios <saida> 20` com o `fs.img` da execução anterior | mediana **8,10 s** (mín. 8,08, máx. 8,19) |
| UEFI, 192 MiB de RAM, disco novo, 2 execuções | `QEMU_MEM=192M tools/qemu-headless.sh uefi <saida> 45` | mediana **8,83 s** (mín. 8,78, máx. 8,88) |

Onde o tempo vai (BIOS, disco novo, mediana): 0,2 s até o relógio calibrado; **2,6 s** montando e formatando o
disco (0,65 s num disco já formatado); **5 s de vinheta de abertura**, que tem duração mínima fixa por desenho
(`splash end (artificial >= 5 s)`); 0,65 s carregando o desktop do disco; 0,16 s pintando o papel de parede.

Firmware e bootloader, antes da entrada do kernel (TSC desde o reset da VM, em TCG): BIOS ~**11,1 s**
(mín. 10,7, máx. 11,2), UEFI ~**5,7 s** (mín. 5,6, máx. 5,9). Esse trecho é dominado pela emulação do
firmware em software. **Com KVM: não medido.**

## Tamanho e memória

| Número | Valor | Fonte e comando |
|---|---|---|
| Imagem BIOS | **8 881 152 bytes** (8,9 MB) | `ls -l target/release/build/os/*/out/kitsune-bios.img` |
| Imagem UEFI | **8 454 144 bytes** (8,5 MB) | idem, `kitsune-uefi.img` |
| Heap em uso, desktop ocioso | **2 263 312 bytes** (2,2 MiB) de um heap fixo de 64 MiB | build `perf-trace`; `QEMU_MEM=128M tools/perf/run.sh <img> bios <saida> 70 tools/perf/scen/idle.sh`, depois `tools/perf/w8-heap.sh <saida>/serial.log` (amostra de 1 s, 15 amostras, mín. 2 263 312, máx. 2 282 832) |
| BSS do kernel (heap incluso) | ~**91 MiB** (`mem_size 0x5b8e4c0` no mapeamento do bootloader) | linha `Mapping bss section` na `serial.log` |
| RAM mínima testada | **128 MiB** em BIOS (a execução ociosa acima usou `QEMU_MEM=128M`); **192 MiB** em UEFI (as execuções de boot UEFI acima usaram `QEMU_MEM=192M`) | os mesmos comandos |
| Atlas de glifos e cache de ícones após o primeiro quadro | 66 131 bytes (924 glifos) e 798 208 bytes | linha `ui: memory after the first frame` da serial |

## Desktop ocioso

Mesma execução `idle.sh` (build `perf-trace`, BIOS, 128 MiB), últimos segundos depois do boot:

| Número | Valor | Comando |
|---|---|---|
| CPU ocupada | **0 %** (`cpu samples idle=250 busy=0` em cada um dos últimos 4 segundos) | `grep -a "cpu samples" <saida>/serial.log \| tail` |
| Quadros por segundo | **1** (o repintar local do relógio, caminho `clockl`) nos últimos 6 segundos | `python3 tools/perf/idle_tail.py <saida> 6` |
| Custo do quadro do relógio | média **0,54 ms**, pior 0,89 ms (13 quadros) | `python3 tools/perf/summ.py <saida>` |

## Quadros

`python3 tools/perf/summ.py <saida>`, build `perf-trace`, BIOS, 1280x720, QEMU TCG. O caminho `animdm` é o quadro
de animação/dano do compositor (o que roda enquanto uma janela se mexe).

| Cenário | Comando | Resultado |
|---|---|---|
| Arrastar uma janela (60 passos de ida e 60 de volta) | `QEMU_MEM=128M tools/perf/run.sh <img> bios <saida> 80 tools/perf/scen/drag.sh` | **6,4 ms** por quadro em média (1311 quadros), pior 43,7 ms |
| Abrir e fechar a Calculadora 4 vezes | idem com `tools/perf/scen/openclose.sh` | **5,5 ms** por quadro em média (706 quadros), pior 60,6 ms |

A máquina de medição é compartilhada e a dispersão é de uns 30 %: `docs/TESTING.md` registra o arrasto entre 4,5 e
7,5 ms conforme a rodada *(de outro documento)*. A frase "~6 ms por quadro" do README segue a medição de hoje.

## Testes e verificação

| Número | Valor | Comando |
|---|---|---|
| Testes do `kitsune_core` | **2953 passam**, 0 falham, 9 ignorados (mais 2 testes de integração e 1 de documentação) | `cargo test -p kitsune_core` |
| Alvos de fuzz | **17** | `ls fuzz/fuzz_targets \| wc -l` (cada alvo roda com `cd fuzz && cargo fuzz run <alvo>`) |
| Cobertura de linhas | **94,7 %** das linhas (40 302 linhas, 2 146 não cobertas), 95,6 % das funções, 93,9 % das regiões; medida bruta, inclui os módulos de teste. O 96,6 % que constava em versões antigas do README era de outra rodada e não foi reproduzido hoje, por isso deixou de ser citado | `cargo llvm-cov -p kitsune_core --summary-only` |
| Blocos `unsafe` do kernel com `// SAFETY:` | **100 %**, imposto pelo lint | `cargo lint-kernel` (falha com `clippy::undocumented_unsafe_blocks`) |
| Boot BIOS e UEFI sem pânico | `boot_ok=1 fatal=0` nos dois modos | `tools/verify-boot.sh <saida>` |

## Outros números citados

| Número | Fonte |
|---|---|
| 250 Hz (frequência do scheduler) | `TIMER_HZ` em `kernel/src/interrupts.rs` e [`ARCHITECTURE.md`](ARCHITECTURE.md); amostras `timer isr n=250` por segundo na serial |
| 46 raízes de certificado | [`design/tls-browser.md`](design/tls-browser.md); `kitsune_core/data/trust-store.sha256` (46 entradas) |
| Teto de 24 MiB de memória, 20 M de instruções por chamada, 4096 KiB de disco e 32 arquivos abertos por app | [`design/apps.md`](design/apps.md) e [`BUILDING.md`](BUILDING.md) (tabela do manifesto) |
| Contraste de texto 15,6:1 (claro) e 12,8:1 (escuro) | [`design/ui-design.md`](design/ui-design.md), §2.1 *(de outro documento)* |
| 8 cores de destaque, 13 apps (9 do sistema e 4 WebAssembly) | `kitsune_core/src/system/settings`, `BUNDLED_APPS` em `kernel/build.rs` |
| Tabela "antes da auditoria / depois" (189 testes, 83 para 250 iterações/s, ~11 ms para ~0,4 ms, 15,6 para 0,2 ms) | [`audit/RELATORIO.md`](audit/RELATORIO.md) *(de outro documento; QEMU sem KVM, proporções)* |
