> **Nota (2026-10):** as medições deste relatório são anteriores às otimizações de disco, catálogo, animações e boot; os números atuais estão em [`../BENCHMARKS.md`](../BENCHMARKS.md) e no [`CHANGELOG.md`](../../CHANGELOG.md). A vinheta de 5 s e o laço ocupado descritos aqui não existem mais.

> *Written when the project was called OSjeff (renamed Kitsune in 2026-10; names and paths below are the historical ones).*

# Auditoria 04 — Desempenho

Branch: `worktree-agent-a0c586cd41464c53b` (parte de `fc89615`). Toolchain `nightly-2026-10-05`.
Escopo: kernel bare-metal (`kernel/`), `osjeff_core`, build. README/ARCHITECTURE.md **não** foram usados como fonte.

## 1. Metodologia e limites (leia antes dos números)

* **Ambiente**: QEMU 8+/TCG **sem aceleração, sem tela**, em host compartilhado com outros agentes (load médio 3–5 em 4 núcleos). Valores absolutos não valem para hardware real; valem **proporções**.
* **Instrumentação** (commit de `trace`): `kernel/src/trace.rs`. Marcos de boot sempre ativos na serial; estatísticas por segundo só com `cargo build --release -p os --features perf-trace` (sem a feature, todo gancho some). Tempo = `rdtsc` calibrado (`perf::calibrate_khz`). Cada quadro é medido em **tempo de parede** e em **tempo de CPU da thread do compositor** (descontando fatias de outras threads).
* **Repetição**: A/B intercalado (build A, build B, A, B…), **3 execuções** por cenário/firmware; reporto **mediana** dos valores por segundo (e min/max). Há ruído de host (frames de 15 ms viram 25/36 ms em múltiplos de ~10 ms: fatias do escalonador do host). Por isso uso mediana e min.
* **Modo `-icount shift=0`** (determinístico, 1 execução): útil para razões entre passos de otimização. Atenção: nele o tempo virtual inclui as fatias ociosas do round-robin (ver achado 1), então os valores são ~3× a contagem de instruções; só as **razões** são usadas.
* **Micro-bench no boot** (`trace::bench_prims`/`bench_alloc`, melhor de 4, `-icount`): ciclos por pixel/chamada, sem interferência de ticks.
* **Screenshot idêntico**: `tools/perf/cmp.sh` compara PNGs pixel a pixel (`compare -metric AE`). Só são mascarados o bloco do HUD (números ao vivo + faixa de sombra que acumula, ver achado 12) e os dígitos do relógio. Cenários: `scen/shots.sh` (idle, 4 janelas, arrasto, menu de contexto, painel iniciar, digitação) e `scen/shots2.sh` (janela cuja sombra toca/cobre o relógio). **AE = 0 em todos os PNGs, BIOS e UEFI**, verificado nas imagens acumuladas "LUT+fill24+relógio local" e "+sombra" (estado final comitado); os passos 1 e 2 não foram fotografados isoladamente (o micro-bench e o estado acumulado cobrem a mesma saída).
* Não medido neste ambiente (precisa de hardware): custo real de VRAM não-cacheada/WC, PIO real do ATA, frequência real do PIT.

## 2. Tabela de medições

Tempos em ms de **parede TCG** salvo indicação. "antes" = `fc89615` + instrumentação; "depois" = todos os commits de otimização desta branch.

| Cenário | antes | depois | Método | Status |
|---|---|---|---|---|
| Quadro do relógio (1×/s, desktop ocioso, BIOS) | mediana 15,6 (min 13,3) | 0,21 | `idle` ×3, 42 quadros | PROVADO |
| idem UEFI | 15,3 | 0,22 | idem | PROVADO |
| Tecla no terminal: quadro completo, BIOS | 26,2 (min 19,8) | 13,3 (CPU real 5,1) | `typing` ×3, 513 quadros | PROVADO |
| idem UEFI | 26,5 | 16,1 (CPU 6,2) | idem | PROVADO |
| Latência IRQ→quadro pronto (digitando, BIOS) | 15,6 | 8,2 | idem | PROVADO |
| CPU ocupada digitando (amostragem 250 Hz, BIOS/UEFI) | 5,3 % / 5,4 % | 2,5 % / 2,9 % | idem | PROVADO |
| CPU ocupada ocioso | 0,79 % | 0,20 % | idem | PROVADO |
| Arrasto de janela, quadro de dano (BIOS) | 2,6 | 1,8 (CPU 1,66) | `drag` ×3, ~1900 quadros | PROVADO |
| Abrir/fechar janela, quadro "rebuild" (BIOS) | 92,8 (pior caso 122) | 75,5 (CPU 27,4) | `openclose` ×3 | PROVADO |
| Micro-bench `fill_rect` 512×320, BIOS (ciclos/pixel) | 14 | 3 | `bench_prims`, icount | PROVADO |
| `fill_round_rect_alpha` 520×324 (ciclos/pixel) | 24 | 10 | idem | PROVADO |
| Texto 8×8 escala 2 (ciclos/caractere) | 1 854 (BIOS) | 1 925 / 2 364 (UEFI) | idem | PROVADO (sem melhora, ver achado 9) |
| Quadro completo (icount, M ciclos virtuais, BIOS) idle / digitando | 27,8 / 36,8 | 0,10 / 4,0 | `steps_*` icount, 1 execução/passo | PROVADO |
| idem UEFI | 25,8 / 28,0 | 0,10 / 4,0 | idem | PROVADO |
| Passos (BIOS, digitando, icount): +LUT / +fill24 / +relógio local / +sombra | 36,8→20,0 / 13,9 / 15,0 / 4,0 | | idem | PROVADO |
| Compositor com 3 threads vs sem threads de trabalho, BIOS (quadro `steady`) | parede 13,4 / CPU 5,2 | 5,06 | `ab2` ×3 | PROVADO |
| idem, latência IRQ→captura | 9,25 | 0,014 | idem | PROVADO |
| idem, abrir/fechar: IRQ→quadro | 67,6 | 17,0 (238 vs 48 quadros de animação em 16 s) | idem | PROVADO |
| Bytes enviados à VRAM por tecla (BIOS) | ≈1,0 MB (janela do terminal enviada 2×) | idem | contador `vram upload` | PROVADO (quanto muda de fato: ≈0,3 KB) |
| Alocações por quadro (idle, digitar, mouse, arrasto, abrir/fechar) | 0 | 0 | contador no alocador | PROVADO |
| Alocação (heap limpa) 64 B / free | 114 / 83 ciclos | — | `bench_alloc` | PROVADO |
| Alocação 4 KiB com 2 000 buracos / free 64 B na lista | 44 115 / 6 081 ciclos | — | idem | PROVADO (O(n)) |
| `web::render` | 0,7 alocações por byte de HTML; 3,7 KB → 2 635 aloc, 190 KB, pico 105 KB; 50 KB → 34 821 aloc, 2,5 MB, pico 1,4 MB | — | `bench/benches/alloc_count.rs` (host) | PROVADO |
| ISR do timer (parte Rust, ciclos TSC TCG) | 16–19 k (3 threads) / 12,7 k (1 thread) | — | `ab2` | PROVADO; dominado pelo `outb` de EOI emulado |
| Gravação ATA da imagem de FS (99 setores PIO) | 45 ms (1 amostra, TCG) | — | `ata write` na 1ª formatação | PROVADO (1 amostra) |
| Leitura ATA no boot (`Desktop::new`) | 36,5 ms (1 amostra) | — | idem | PROVADO (1 amostra) |
| Tamanho do ELF do kernel | 1 865 488 B (≈370 KB não carregáveis: `.symtab`/`.strtab`) | 1 770 904 B (com instrumentação desligada) | `ls`, `llvm-size` | PROVADO |
| Imagem BIOS / UEFI | 4 686 848 / 4 259 840 B | idem (arredonda em MiB) | `ls` | PROVADO |
| Variações de profile (`panic=abort`, `strip`, LTO thin/off, opt-level s/z) | — | — | — | **NÃO MEDIDO** (prazo). Hipótese: `panic=abort` já é o padrão do alvo `x86_64-unknown-none` (rustc_info mostra `panic="abort"`), logo binário igual; `strip=symbols` remove ≈370 KB do ELF (≈20 %) e reduz a leitura do bootloader; LTO fat→thin troca ~1 min de build por tamanho/velocidade a medir. Como medir: `CARGO_PROFILE_RELEASE_STRIP=symbols cargo build --release -p os --features perf-trace` e comparar `ls` + marcos de boot + `bench` |
| Criterion (`osjeff_core` e primitivas no host) | — | — | `cd bench && cargo bench` | **NÃO EXECUTADO** (crate compila; só `alloc_count` rodou). Hipótese: terminal/editor/calc/keymap em dezenas de ns; `web::render` domina |
| Navegador com página real | — | — | — | **NÃO MEDIDO**: o fetch HTTP ao host via SLIRP falhou também no baseline ("Falha ao carregar a página"); medi só a página inicial (quadro `steady` com o navegador aberto ≈ 25 ms de parede) |
| Write-combining / PAT | — | — | — | **NÃO MEDIDO** (TCG não distingue UC/WC); análise no achado 5 |

### Linha do tempo de boot (BIOS, mediana de 12 execuções; UEFI entre parênteses)

| Marco | ms desde `kernel entry` | passo |
|---|---|---|
| Firmware + bootloader antes do kernel (TSC desde o reset) | 3 669 (3 755) | — |
| `framebuffer cleared` | 2,0 | 2,0 |
| `heap init + smoke test` | 3,2 | 1,3 |
| **`wasm demo done`** | 41,8 (44,7) | **38,5** |
| PCI + ATA detect + PS/2/IDT/PIT | 44,7 | 2,5 |
| **`tsc calibrated`** (25 ticks do PIT, espera ocupada) | 149,2 | **104,1** |
| NE2000 + DHCP + spawn threads | 172,9 | 23,7 (DHCP ≈1 ms no SLIRP; sem servidor, espera até 300 ms por fase) |
| **splash** (artificial) | 4 409 (4 936) | **4 235 (4 774)** |
| wallpaper | 4 428 | 15,4 |
| `Desktop::new` (lê FS do ATA, 2×IDENTIFY) | 4 544 | 105,5 |
| primeiro quadro | 4 590 (5 111) | 59,8 |

Tempo real de init do kernel ≈ 0,18 s; o resto é splash (4,1–5,2 s: o loop usa segundos inteiros do RTC, então dura 4–5 s, **não** "≥ 5 s" como o comentário diz) e firmware/bootloader (3,5–3,9 s no TCG).

## 3. Respostas objetivas às perguntas do escopo

* **Cópia back→framebuffer**: por linha/bloco com `copy_from_slice` (`fb_full_blit`, `blit_rect`). Não há conversão por pixel: o buffer `back` já tem o layout do framebuffer (Bgr, 3 B/pixel no BIOS).
* **Preenchimentos**: o caminho de 3 bytes fazia 3 stores com bounds check por pixel (14 ciclos/px); o de 4 bytes já era `fill` de u32. Blend alfa fazia 6 acessos com bounds check por pixel.
* **Lê VRAM?** Sim, em um ponto: o HUD (`perf::draw`) desenha com `fill_round_rect_alpha` direto no framebuffer, lendo e escrevendo VRAM 10×/s (ver achado 6). O cursor só escreve (`put`).
* **Retângulos sujos**: não são fundidos; o caminho "steady" envia janela focada + janela anterior + relógio, e quando o foco não mudou envia a mesma janela duas vezes (achado 7).
* **Fonte**: renderiza bit a bit a cada quadro (cada pixel aceso vira `fill_rect`), sem cache de glifos: ≈1,9 k ciclos/caractere.
* **Sombras**: recalculadas a cada recomposição, 2 camadas por janela, ≈346 k pixels por janela de 512×320 (achado 2).
* **Alocador**: o caminho quente do compositor **não aloca** (0 alocações em todos os cenários medidos), logo o alocador não é o gargalo do desktop. Só `web::render`, a pilha de rede e o app WASM (19 aloc/s) alocam.
* **ATA PIO**: espera ocupada com limite de 1 M iterações por espera (até ≈1 s se o disco travar), **bloqueia o compositor** durante o flush e reescreve a imagem inteira (99 setores) a cada salvar/lixeira/mkdir (achado 8).
* **Scheduler**: `hlt` quando ocioso (CPU ≈99,8 % ociosa em amostragem), PIT 250 Hz; ver achado 1 (problema real).

## 4. Achados

### [MÉDIA] 1. Threads ociosas consomem a fatia inteira em `hlt`: o compositor roda a 1/3 da velocidade
Onde: `kernel/src/sched.rs:176-205` (round-robin), `kernel/src/fetch.rs:92-94`, `kernel/src/wasm/mod.rs:446-455`
O que acontece: com `fetcher` e `wasmapp` sempre no round-robin, cada um chama `hlt` quando não tem trabalho e fica parado até o fim de sua fatia de 4 ms.
```rust
let next = (cur + 1) % n;          // sched.rs: sempre visita todas as threads
...
} else { x86_64::instructions::hlt(); }   // fetch.rs: não cede a fatia
```
Por que importa: durante um quadro longo o compositor só recebe 1 de cada 3 fatias. Quadro `steady` = 13,4 ms de parede para 5,2 ms de CPU; a latência IRQ→captura média é 9,25 ms; as animações têm 5× menos quadros (48 vs 238 em 16 s). O HUD mostra tempo de parede, então superestima o custo ≈2,6×.
Como provar: PROVADO. `ab2` (3 execuções, BIOS): build A (3 threads) vs build B sem as threads de trabalho: `steady` 13,4 → 5,06 ms (= CPU de A), pickup 9,25 ms → 0,014 ms, e2e 8,3 → 2,6 ms. O campo `cpu=` do trace vem da contabilidade de CPU por thread em `switch_current`.
Correção proposta: "estacionar" workers ociosos (`sched::park_current`/`unpark`; o escalonador pula slots estacionados; o compositor nunca estaciona; reconferir a condição de trabalho depois de estacionar para não perder o acordar). Implementei e testei funcionalmente (app WASM abre e anima, 120 draws/s; o worker do fetch acorda, aparece `fetch: GET ...`), **mas não fiz A/B de 3 execuções do build com a correção**, só o proxy "sem threads" acima; por isso **não comitei**: diff em `perf-upload-dedupe-and-park-workers.patch` (scratchpad do agente D).
Esforço: baixo/médio. Severidade: MÉDIA.

### [MÉDIA] 2. Sombras de janela: ≈346 k pixels de blend por janela a cada recomposição
Onde: `kernel/src/desktop/render.rs:198-204`, `kernel/src/fb.rs` (`fill_round_rect_alpha`)
O que acontece: duas camadas alfa (520×324 e 536×332) por janela, calculadas a 24 ciclos/pixel e desenhadas também por baixo do corpo opaco, que depois as sobrescreve.
```rust
for &(off, exp, a) in &[(6usize, 4usize, 28u16), (14, 12, 14)] {
    c.fill_round_rect_alpha(sx, sy, w + exp * 2, h + exp, 14 + exp, theme::SHADOW, a);
}
```
Por que importa: era ≈97 % de um quadro de recomposição (alfa = 27,0 de 27,8 M ciclos virtuais no quadro do relógio) e cresce com o número de janelas (HUD mostrou 228 ms/quadro com 4 janelas no TCG).
Como provar: PROVADO (`prim alpha`, `bench_prims`, `steps_*`).
Correção proposta (implementada, comitada): tabela 3×256 para áreas grandes (24→10 ciclos/px, mesma fórmula) e não misturar a sombra sob as linhas totalmente opacas do corpo (≈85 % menos pixels). Saída bit a bit idêntica. Quadro completo de digitação (icount): 36,8 → 20,0 (LUT) → 4,0 (com o resto) M; screenshots AE=0.
Esforço: baixo. Severidade: MÉDIA. Commits: ver §6.

### [MÉDIA] 3. Tick do relógio recompunha a cena inteira só para redesenhar 124×42 px
Onde: `kernel/src/main.rs` (ramo `clock_tick`), `kernel/src/desktop/mod.rs`
O que acontece: 1×/s `copy_bg(back)` (2,7 MB) + `desk.render()` de todas as janelas e sombras, para enviar à VRAM apenas o retângulo do relógio.
Por que importa: quadro de 15,6 ms de parede por segundo (travadinha periódica, e acorda a CPU).
Como provar: PROVADO (`idle` ×3: 15,6 → 0,21 ms; UEFI 15,3 → 0,22 ms; CPU ocupada ociosa 0,79 % → 0,20 %).
Correção (implementada, comitada): se nada anima, não há overlay, o Task Manager está fechado e nenhuma janela (sombra incluída, 32 px de folga) alcança a pílula, restaurar o wallpaper sob a pílula e redesenhá-la. Caso contrário, caminho antigo. Testado com janela cuja sombra encosta e que cobre a pílula (`shots2.sh`), AE=0.
Esforço: baixo. Severidade: BAIXA/MÉDIA.

### [BAIXA] 4. Preenchimento de retângulos em 24 bpp byte a byte
Onde: `kernel/src/fb.rs` `fill_rect_inner` (caminho `bpp == 3`)
O que acontece: 3 stores com bounds check por pixel; o BIOS/VBE usa 3 B/pixel.
Por que importa: 14 ciclos/px; corpo de janela, cabeçalhos e todo fundo de widget.
Como provar: PROVADO (micro-bench 14 → 3 ciclos/px; UEFI já 1).
Correção (comitada): 4 pixels (12 B) por store a partir de padrão. Idêntico.
Esforço: baixo. Severidade: BAIXA.

### [MÉDIA] 5. Framebuffer mapeado como memória cacheável comum (sem PAT/WC)
Onde: bootloader `bootloader-x86_64-common` (`lib.rs:303-304`: flags `PRESENT|WRITABLE|NO_EXECUTE`); kernel não toca em PAT/MTRR (`grep` por `Msr|PAT|mtrr` só acha `Cr3` em `virtio.rs`).
O que acontece: PAT índice 0 = WB; o tipo efetivo é combinação com o MTRR da faixa: em hardware real o PCI hole costuma ser UC, então escritas na VRAM seriam não-combinadas.
Por que importa: um quadro cheio BIOS = 2,76 MB; só o `Settle`/animação enviam tudo, mas cada tecla envia ≈0,5–1 MB (achado 7).
Como provar: SUPOSIÇÃO. Não mensurável no TCG. Estimativa (não medida): UC ≈ 10–50 MB/s ⇒ 2,76 MB ≈ 55–280 ms e 0,5 MB/tecla ≈ 10–50 ms; WC ≈ GB/s ⇒ <1 ms.
Correção proposta: o kernel já tem `physical_memory_offset` e usa `OffsetPageTable` (`virtio.rs`). Programar IA32_PAT (MSR 0x277) entrada 1 = WC, ligar `WRITE_THROUGH` (PWT) nas PTEs de 4 KiB do framebuffer (o bootloader mapeia página a página), `invlpg`, e usar `sfence` após cada upload. Verificar com CPUID que PAT existe. Validar em hardware (taxa de `copy_from_slice` de 2,7 MB antes/depois). Não precisa mudar o bootloader.
Esforço: médio. Severidade: MÉDIA (potencialmente a maior em hardware real).

### [MÉDIA] 6. HUD lê e escreve VRAM e desenha texto direto nela, 10×/s
Onde: `kernel/src/main.rs` (bloco do HUD), `kernel/src/perf.rs:64-105`
O que acontece: `perf.draw(&mut Canvas sobre framebuffer.buffer_mut())` chama `fill_round_rect_alpha` (lê cada byte da VRAM e regrava) e `font::draw_text` (stores de 2×2 pixels direto na VRAM).
```rust
let mut c = Canvas::new(&mut framebuffer.buffer_mut()[..n], info);
perf.draw(&mut c, heap_pct, sched::thread_count());   // blend lê VRAM
```
Por que importa: ≈12,6 k pixels × 3 bytes = 38 KB lidos da VRAM por refresh; leitura de VRAM não-cacheada é a operação mais lenta possível.
Como provar: leitura de VRAM PROVADA por inspeção de código; custo em hardware SUPOSIÇÃO (38 k leituras de byte a 0,1–1 µs ≈ 4–38 ms por refresh, 10×/s).
Correção proposta: compor o HUD numa cópia pequena em RAM (snapshot da região de `back`, desenhar, enviar um retângulo, restaurar). Não implementei: a saída não seria idêntica por causa do achado 12 (faixa de sombra que hoje acumula) e o ganho só aparece em hardware.
Esforço: baixo. Severidade: MÉDIA.

### [MÉDIA] 7. Cada tecla envia a janela inteira duas vezes à VRAM; sem diff nem fusão de retângulos
Onde: `kernel/src/main.rs` (`up(fb_)`, `up(pf)`, `up(clock)` no caminho `scene_dirty`)
O que acontece: `focused_box()` e `prev_focused` são o mesmo retângulo quando o foco não muda, e ambos são copiados inteiros.
Por que importa: ≈1,0 MB/s·tecla de upload para ≈0,3 KB que realmente mudam (0,03 %).
Como provar: PROVADO (contador `vram upload=…B changed=…B`, digitação BIOS: 4–7 MB/s enviados, 1,5–20 KB/s alterados).
Correção proposta: (a) não reenviar `pf` quando `Some(pf) == focused` (2 linhas, metade dos bytes; escrita pronta, **sem A/B pós-correção**, no `.patch`); (b) manter cópia em RAM do último quadro enviado e enviar só spans que mudaram (esforço médio; ganho dominante em VRAM lenta).
Esforço: baixo (a) / médio (b). Severidade: MÉDIA.

### [BAIXA] 8. Flush do FS reescreve 99 setores por PIO e bloqueia o compositor
Onde: `kernel/src/ata.rs:253-290`, `kernel/src/desktop/mod.rs:57` (`flush_disk`)
O que acontece: cada salvar/mkdir/lixeira escreve `fs::IMAGE_SIZE` = 50 308 B (4 + 48×1048) com `outw` em laço, mais `CMD_FLUSH`.
```rust
for w in 0..(SECTOR / 2) { outw(REG_DATA, lo | (hi << 8)); }
```
Por que importa: 45 ms (TCG, 1 amostra) por operação, com a interface parada; em disco real o `FLUSH CACHE` soma milissegundos. Uma entrada de arquivo ocupa 3 setores.
Como provar: PROVADO para a contagem de setores e o tempo no TCG (1 amostra); custo em disco real SUPOSIÇÃO.
Correção proposta: marcar setores sujos em `osjeff_core::fs` e gravar só os alterados (≈30× menos); mover o flush para uma thread. Esforço médio. Severidade: BAIXA.

### [BAIXA] 9. Texto: 1,9 k ciclos por caractere, sem cache de glifos
Onde: `kernel/src/font.rs:draw_char`
O que acontece: para cada bit aceso chama `fill_rect(scale×scale)` com todas as checagens.
Por que importa: terminal cheio ≈ 360 caracteres ≈ 0,7 M ciclos ≈ 1/3 do quadro de digitação já otimizado.
Como provar: PROVADO (`bench_prims`: 1 854 BIOS / 2 364 UEFI ciclos/caractere; a mudança de fill de 24 bpp não ajuda porque os blocos são 2×2).
Correção proposta: rasterizar o glifo direto (offset base + stores por linha) ou cache de bitmap por (char, escala); saída idêntica; esperado 2–3×. Não implementado. Esforço baixo/médio.

### [BAIXA] 10. Alocador de lista livre O(n)
Onde: `kernel/src/allocator.rs:172-186` (`find_region`), `add_free_region`
Como provar: PROVADO. Heap limpa: 114/83 ciclos; com 2 000 buracos: alloc de 4 KiB 44 115 ciclos, free 12 083, free de 64 B no meio da lista 6 081. `web::render` faz 0,7 alocações por byte de HTML (50 KB ⇒ 35 k alocações), então a pior fragmentação vem do navegador. Não afeta o desktop (0 alocações por quadro).
Correção proposta: listas por classe de tamanho (bins) para ≤ 4 KiB. Esforço médio. Severidade: BAIXA.

### [BAIXA] 11. Worker WASM renderiza sem pacing; `rtc::now()` e `free_bytes()` a cada iteração
Onde: `kernel/src/wasm/mod.rs:446-480`, `kernel/src/main.rs` (`rtc::now()` no topo do laço; `ALLOCATOR.free_bytes()` a cada HUD)
O que acontece: com a janela WASM aberta o worker chama `render` em laço contínuo (≈123 de 250 amostras de CPU); o compositor só consome ≈22–120 quadros/s. `rtc::now()` faz ≥8 acessos a porta + espera de UIP por iteração.
Como provar: PROVADO (amostragem de CPU: `wasm=123 comp=11`). Custo de `rtc`/`free_bytes` não medido (hipótese: <0,2 %).
Correção proposta: pacing por tick no worker (muda a velocidade de jogos que avançam por `render`, por isso não aplicado); ler o RTC 1×/tick. Esforço baixo. Severidade: BAIXA.

### [BAIXA] 12. Defeitos visuais encontrados durante os testes (não são de desempenho)
Onde: `kernel/src/main.rs` (HUD) e caminho `scene_dirty` do estado estável
* A faixa de sombra do HUD (linhas y 72–76, fora do retângulo restaurado `hr`) recebe um blend a cada refresh e escurece com o tempo (diferença entre execuções no `compare`; por isso o HUD é mascarado).
* Depois de fechar o menu de contexto com um clique, parte inferior do menu fica "fantasma" na tela (`s4/s5` do `shots.sh`): o caminho "steady" só envia janela focada + relógio e não cobre o retângulo do overlay fechado.
Como provar: PROVADO (screenshots em `tools/perf/scen/shots.sh`, fantasma visível em `s5_start.png`). Correção proposta: incluir `overlay_bounds()` anterior nos uploads. Severidade: BAIXA.

### [BAIXA] 13. Boot: itens evitáveis
* `wasm::run_demo()` no boot: 38,5 ms (≈21 % do init real do kernel; inclui imprimir 50 caracteres na serial). Mover para depois do primeiro quadro ou atrás de feature de debug.
* Calibração do TSC: 104 ms de espera ocupada (25 ticks do PIT); pode ser feita durante o splash ou com CPUID leaf 0x15.
* DHCP: espera até 300 ms por fase quando não há servidor; sem retransmissão (`dhcp_acquire`).
* Splash: 4–5 s artificiais, 100 % de CPU (quadro completo redesenhado e enviado à VRAM a cada iteração + `delay_cycles(20 M)`), barra "suave" reiniciada a cada segundo do RTC.
* ELF com ≈370 KB de `.symtab/.strtab` e BSS de 95,5 MB (3 buffers de 8 MiB + heap de 64 MiB) zerado pelo bootloader.
Como provar: marcos de boot PROVADOS (tabela acima); custo do BSS em hardware SUPOSIÇÃO. Severidade: BAIXA.

## 5. Como reproduzir

```
cargo build --release -p os --features perf-trace
tools/perf/run.sh <imagem> bios|uefi <saida> 150 tools/perf/scen/typing.sh
tools/perf/ab.sh <rotulo> <dirA> <dirB> 3 "bios uefi" idle typing openclose drag
python3 tools/perf/agg.py <rotulo>        # medianas A/B (parede e CPU)
python3 tools/perf/boot.py <runs...>      # marcos de boot
tools/perf/cmp.sh <dirA> <dirB> bios      # screenshots pixel a pixel
cd bench && cargo bench                   # criterion no host
```
Imagens vão para `target/release/build/os/*/out/osjeff-*.img`; `-icount shift=0` via `QEMU_EXTRA`.

## 6. Commits desta branch

| Commit | Assunto |
|---|---|
| `908d91d` | perf(trace): boot milestones + opt-in runtime stats on serial |
| `0c7304c` | perf(tools): QEMU scenario harness and host criterion benches |
| `903206a` | perf(fb): table-driven alpha blend for large areas (achado 2) |
| `ff7cc53` | perf(fb): store four 24-bit pixels per step in fill_rect (achado 4) |
| `7220733` | perf(desktop): repaint only the clock pill on the per-second tick (achado 3) |
| `41a4bb8` | perf(desktop): do not blend window shadows under the opaque body (achado 2) |

Cada commit de kernel passou `cargo test -p osjeff_core` (189 testes), `cargo clippy -p kernel --target x86_64-unknown-none -- -D warnings` (apenas o aviso-base `render.rs:74`) e boot BIOS. Fora dos commits (sem A/B pós-correção): não reenviar a janela duas vezes (achado 7a) e estacionar workers ociosos (achado 1); diff em `perf-upload-dedupe-and-park-workers.patch` no scratchpad do agente.
