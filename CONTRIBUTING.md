# Contribuindo com o Kitsune

Este projeto é de um desenvolvedor só, mas trabalha como se não fosse: toda mudança
precisa de prova. Estas regras são as que foram usadas na auditoria de desempenho
e segurança (`docs/audit/`).

## Licença e termos de contribuição

O código é **disponível para leitura** sob a [PolyForm Strict 1.0.0](LICENSE) e pertence ao
Titular (Jeferson Reis Almeida). Você pode estudá-lo e usá-lo para fins não comerciais, mas **não**
pode redistribuí-lo, criar sistemas derivados nem vendê-lo. Quem quer contribuir recebe uma
permissão limitada para modificar o código com esse fim e cede os direitos da contribuição ao
Titular: leia [`LICENSE-CONTRIBUTORS.md`](LICENSE-CONTRIBUTORS.md) antes do primeiro pull request.
**Todo commit precisa de `Signed-off-by`** (`git commit -s`), que registra o aceite desses termos.
Uso comercial exige licença escrita do Titular.

## Antes de começar

```bash
rustup show                 # instala o nightly fixado e os alvos
cargo test-core             # ~3430 testes, ~1 min
cargo lint-kernel && cargo lint-host
```

Veja [`docs/BUILDING.md`](docs/BUILDING.md) para QEMU/OVMF e
[`docs/TESTING.md`](docs/TESTING.md) para as camadas de verificação.

## Fluxo

1. **Uma mudança por commit.** Cada um pode ser revertido sozinho. Mensagem em
   inglês, no estilo do histórico: `fix(net): ...`, `perf(fb): ...`,
   `refactor(core): ...`, `docs(audit): ...`. O corpo explica o *porquê* e **como foi
   verificado**.
2. **Nada muda sem teste.**
   - Lógica pura (parser, regras de janela, FS, rede) → `kitsune_core`, com teste
     unitário que **falha antes e passa depois**.
   - Mudança no `kernel/` → `tools/verify-boot.sh` (BIOS **e** UEFI) com o desktop
     idêntico à baseline, ou um screenshot explicando a diferença intencional.
3. **Número antes/depois** para qualquer afirmação de desempenho. QEMU sem KVM só
   vale como proporção; repita a medição e diga a dispersão.
4. **Evidência.** Todo achado com `arquivo:linha` e o trecho. Não repita o README: ele
   já esteve errado (testes e cobertura desatualizados).

## Antes de abrir um PR

```bash
cargo fmt --all -- --check
cargo test-core
cargo lint-kernel
cargo lint-host
cargo deny check
tools/verify-boot.sh /tmp/kit-novo /tmp/kit-baseline    # se tocou em kernel/
```

O CI roda os quatro primeiros. Mudou o `Cargo.lock`, o `bootloader` ou o nightly?
Isso é uma decisão deliberada: explique no commit (um `nightly` sem data já
quebrou o build uma vez, ver `docs/audit/RELATORIO.md`).

## Regras de código

- **`kitsune_core` é `forbid(unsafe_code)` e `no_std`.** Não adicione `std`.
- **Todo `unsafe` leva `// SAFETY:`** com a invariante *real* e por que ela vale ali.
  O kernel liga `clippy::undocumented_unsafe_blocks`; com `-D warnings` o lint falha
  sem o comentário. Se o tipo **não** garante a invariante (por exemplo uma função
  segura que devolve `&'static mut`), diga isso no comentário em vez de fingir.
- **Estado global mutável** passa por `RacyCell` (`kernel/src/sync.rs`). É o mesmo
  padrão de `static mut` com outro nome: documente qual thread/IRQ acessa cada um.
- **Handlers de IRQ não alocam e não pegam lock.** O allocator usa spin lock com
  interrupções desligadas, mas um ISR que aloque ainda é bug.
- **Dado externo nunca é confiável.** Em parser de rede, disco, HTML/CSS ou `.wasm`:
  `get()`/`checked_*`/`saturating_*` em vez de indexar e somar. O build de release
  do kernel **não** tem `overflow-checks`, então um estouro dá a volta em silêncio.
- **Não aumente a superfície de privilégio.** Tudo roda em ring 0. Qualquer coisa
  que execute código de terceiros passa pelo WASM (com limites) e não pelo kernel.
- **Dependências novas no `kernel`** precisam de justificativa (tamanho, `no_std`,
  `unsafe`, licença) e passam por `cargo deny check`.

## Onde coloco X? (Where do I put X)

| Quero... | Vai para |
|---|---|
| Lógica pura (parser, regra, formato), testável no host | `kitsune_core/src/<grupo>/` (veja a tabela de grupos em [`docs/design/code-structure.md`](docs/design/code-structure.md)), com `tests.rs` ao lado |
| Pixels, entrada e glue de hardware de um app | `kernel/src/desktop/apps/<app>/` (`state`, `input`, `paint`) |
| Um widget ou helper de desenho compartilhado | `kernel/src/desktop/kit/` |
| Painel, barra de apps, menus, Apps/Busca, toasts | `kernel/src/desktop/shell/` |
| Moldura, ciclo de vida, snap e arrasto de janela | `kernel/src/desktop/windows/` |
| Teclado e ponteiro (roteamento) | `kernel/src/desktop/input/` |
| Acesso a arquivos pelo desktop, trait do sistema, thread do shell | `kernel/src/desktop/services/` |
| Um driver | `kernel/src/<driver>.rs` + a parte pura em `kitsune_core/src/hw/` |
| Um app WASM de exemplo (fora da imagem) | `wasm-apps/examples/<app>/` |
| Texto visível ao usuário | chave em `assets/i18n/{pt,en}.txt` + `t!("chave")` |
| Teste de um módulo | `<módulo>/tests.rs` (ou `tests/` por assunto); auxiliar compartilhado em `kitsune_core/src/testutil.rs` |

Checklists de "novo app / tipo de janela / overlay / driver / módulo do core" estão em
`docs/design/code-structure.md`. Antes de abrir o PR: `cargo test -p kitsune_core structure::` confere as
regras de dependência entre as pastas do core.

## Decisões de arquitetura

Mudanças grandes (modo usuário, paginação por processo, novo allocator, troca de
bootloader) viram um ADR em `docs/audit/` antes de virar código. O estado atual é
o [`adr-isolamento.md`](docs/audit/adr-isolamento.md).

## Segurança

Achou uma falha? Leia [`SECURITY.md`](SECURITY.md) antes de abrir uma issue pública.
