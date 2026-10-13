# Compilar e rodar

Este guia leva do `git clone` a um desktop Kitsune na tela. O que ele pede do **host** (a
máquina em que você compila e roda o QEMU) está na seção 1; o Kitsune em si não depende de
nenhum sistema do host. Todos os comandos foram executados de verdade nesta árvore.

## 1. Requisitos do host

| O quê | Para quê | Observação |
|---|---|---|
| [`rustup`](https://rustup.rs) | toolchain | O `rust-toolchain.toml` fixa **`nightly-2026-10-05`** e baixa sozinho `rust-src`, `llvm-tools`, `clippy`, `rustfmt` e os alvos `x86_64-unknown-none` e `wasm32-unknown-unknown`. Não use `nightly` solto: ver [Problemas comuns](#5-problemas-comuns). |
| QEMU (`qemu-system-x86_64`) | rodar a imagem | Não precisa de KVM nem de tela para os testes (`-display none`). |
| OVMF | boot UEFI | Ubuntu/Debian: `apt install ovmf`. Só é necessário para `uefi`. |
| `socat` e ImageMagick | harness headless | Só para `tools/qemu-headless.sh` e `tools/verify-boot.sh` (screenshots). |
| `wasi-sdk` | apps C em WASM | Opcional (DOOM e `cdemo`). Os apps de exemplo são Rust e não precisam. |

Ubuntu/Debian de uma vez:

```bash
sudo apt install qemu-system-x86 ovmf socat imagemagick
```

## 2. Compilar a imagem

```bash
cargo build --release -p os
```

O crate `os` embute o kernel como *artifact dependency* (`-Z bindeps`, já
habilitado em `.cargo/config.toml`) e gera duas imagens de disco em
`target/release/build/os-*/out/`: `kitsune-bios.img` (~8,9 MB) e
`kitsune-uefi.img` (~8,5 MB). A primeira compilação leva ~1–2 min (baixa e compila
o bootloader); as seguintes ~25 s.

## 3. Rodar

### Hosts com shell POSIX (Linux, WSL, macOS)

```bash
tools/run.sh            # BIOS, janela do QEMU
tools/run.sh uefi       # UEFI (usa OVMF)
tools/run.sh bios -- -accel kvm   # com KVM, bem mais rápido
QEMU_AUDIO=pa tools/run.sh        # som pelo PulseAudio (pipewire, alsa, sdl, coreaudio...)
QEMU_AUDIO=wav:/tmp/som.wav tools/run.sh   # grava o que o sistema toca num WAV
QEMU_AUDIO=none tools/run.sh      # sem placa de som
```

**Som.** Os dois scripts põem uma placa **AC'97** (`-device AC97`): o kernel loga `audio: AC'97 up` e o Terminal toca
(`tone 440 1000`, `play arquivo.wav`, `volume 50`). Sem a placa o boot é o mesmo (`audio: no device`). Projeto em
[`design/audio.md`](design/audio.md).

O disco do filesystem (`kitsune-fs.img`) é criado na primeira execução e persiste entre
boots. Desde o OJFS v3 ele tem **64 MiB**, mas é um arquivo *esparso* (`truncate -s 64M`;
`run.ps1` usa `SetLength(64MB)`): quase nada ocupa o disco do host. O OJFS v3 exige pelo
menos 1 MiB. Um `kitsune-fs.img` antigo de 64 KiB continua aceito: o boot loga
`storage: disk too small for OJFS v3 (64 KiB), staying on v2` e segue com o v2 como antes.
Para migrar um disco antigo, aumente-o (`truncate -s 64M kitsune-fs.img`): a imagem v2 dos
primeiros 64 KiB é migrada para o v3 no boot seguinte e esses 64 KiB não são alterados.
`FS_SIZE=64K tools/run.sh` (ou apagar o arquivo e criá-lo de 64 KiB) reproduz o disco antigo.

> `cargo run -p os -- uefi` só funciona se `OVMF_PATH` apontar para um firmware
> **único** (`OVMF.fd`). O Ubuntu moderno distribui o OVMF dividido em
> `CODE`+`VARS`, que exige `pflash`; o `tools/run.sh` já cuida disso.

### Windows nativo (sem WSL)

Instale o Rust para Windows ([rustup-init.exe](https://rustup.rs), com as ferramentas C++ do
Visual Studio Build Tools se o instalador pedir) e o QEMU em `C:\Program Files\qemu`. Com
`cargo` no PATH do Windows, `.\run.ps1` compila direto no Windows (`-Native` força, `-Wsl`
volta ao WSL). O `rust-toolchain.toml` baixa o nightly fixado sozinho. Este caminho ainda não
foi exercitado em uma máquina Windows real: se falhar, o WSL continua sendo o caminho testado.

### Windows com aceleração (WHPX)

```powershell
.\run.ps1              # compila no WSL e roda com WHPX
.\run.ps1 -NoAccel     # TCG (software)
.\run.ps1 -SkipBuild   # só boota a imagem existente
.\run.ps1 -Usb         # gera kitsune-uefi.img para gravar em pendrive
.\run.ps1 -Doom        # variante com DOOM (ver §4)
.\run.ps1 -NoAudio     # sem a placa de som
```

O `run.ps1` liga o som por padrão com `-audiodev dsound,id=snd0 -device AC97,audiodev=snd0` (DirectSound do Windows),
se o QEMU instalado tiver os dois; senão avisa e sobe sem som.

### Sem tela (CI, servidor, nuvem)

```bash
# sobe, espera 25 s, salva serial.log e screen.png em /tmp/kit
tools/qemu-headless.sh bios /tmp/kit 25
QEMU_MEM=256M tools/qemu-headless.sh uefi /tmp/kit-uefi 40

# build + boot BIOS e UEFI + comparação do desktop com uma baseline
tools/verify-boot.sh /tmp/kit-novo /tmp/kit-baseline
```

Para mandar teclas e mouse enquanto roda:
`echo "sendkey a" | socat - UNIX-CONNECT:$(cat /tmp/kit/mon.path)`
(o socket fica em `/tmp` porque caminhos de socket Unix têm limite de 107 bytes).

## 4. Apps WebAssembly

A imagem embute os apps `clock`, `notes`, `paint` e `snake` (em `wasm-apps/`;
`kernel/build.rs` compila todos para
`wasm32-unknown-unknown`) e os instala em `/apps` no primeiro boot, sem sobrescrever
os que já existem. (Os exemplos `hello`, `plasma` e `nettest`, em `wasm-apps/examples/`, continuam compiláveis mas não vêm na imagem.) Eles aparecem no launcher Apps (com ícone e nome) e no Gerenciador
de arquivos (vista **Apps**: `Enter` abre, `I` instala, `Del` remove). O ícone "W" do dock
abre o `snake`. Detalhes: [`docs/design/apps.md`](design/apps.md).

Variantes opcionais para o app "legado" do dock (sem manifesto, ABI v1):

| Compilação | O ícone "W" do dock abre |
|---|---|
| padrão | `snake` (empacotado, como os outros) |
| `WASI_SDK_PATH=...` | `cdemo` (C freestanding) |
| `DOOM=1 WASI_SDK_PATH=...` | DOOM ([doomgeneric](https://github.com/ozkl/doomgeneric), GPLv2) |

DOOM **não** vem pronto no checkout: precisa do `wasi-sdk`, de rede para clonar o
doomgeneric (`tools/build-doom.sh`) e de um WAD (`doom1.wad`, não redistribuído; o
Freedoom serve). Use 512 MB de RAM no QEMU.

### Como criar um app

Um app é **um arquivo `.wasm`**: o código mais uma seção `kitsune.manifest` (e, se quiser,
`kitsune.icon`). Passo a passo com o SDK (`wasm-apps/sdk`, `no_std`, sem dependências):

1. **Crie a crate** em `wasm-apps/meuapp/` (workspace isolado, como as outras):

   ```toml
   # wasm-apps/meuapp/Cargo.toml
   [package]
   name = "meuapp"
   version = "0.1.0"
   edition = "2021"

   [lib]
   crate-type = ["cdylib"]

   [dependencies]
   kitsune-sdk = { path = "../sdk" }

   [profile.release]
   opt-level = "s"
   lto = true
   panic = "abort"

   [workspace]
   ```

2. **Escreva o app** (`src/lib.rs`): implemente `App` (só `new` e `render` são
   obrigatórios) e declare o manifesto:

   ```rust
   #![no_std]
   use kitsune_sdk::*;

   manifest!("id=meuapp\nname=Meu App\nversion=1.0.0\nfs=own\nwin_w=420\nwin_h=300\n");
   icon!(include_bytes!("../icon.png")); // opcional: PNG de até 64x64

   struct Meu { toques: u32 }
   impl App for Meu {
       fn new() -> Self { Meu { toques: 0 } }
       fn on_key(&mut self, _code: i32, _mods: i32) { self.toques += 1; }
       fn render(&mut self, c: &mut Canvas) {
           c.clear(0x10141F);
           c.text(16, 16, "Ola!", 0xFFFFFF, 2);
       }
   }
   export_app!(Meu);
   ```

3. **Manifesto** (`chave=valor` por linha; `id`, `name` e `version` são obrigatórios):
   permissões `fs=none|own|home`, `net=none|http|tcp`, `clipboard=none|rw`; quotas
   `mem_mib` (até 24), `fuel_frame` (até 20 000 000 instruções por chamada), `disk_kib` (até
   4096), `max_fds` (até 32); `tick_ms` (período de `on_tick`); janela `win_w`/`win_h`
   (área de conteúdo), `win_min_w`/`win_min_h`, `resizable`. Valor acima do teto, chave
   repetida ou desconhecida, permissão inexistente: o instalador **recusa** o app.
   Tabela completa em [`docs/design/apps.md`](design/apps.md) §2.

4. **Compile e teste isolado**: `cd wasm-apps/meuapp && cargo build --release --target
   wasm32-unknown-unknown` gera `target/wasm32-unknown-unknown/release/meuapp.wasm`.

5. **Embuta na imagem**: acrescente `"meuapp"` em `BUNDLED_APPS` de `kernel/build.rs` e rode
   `cargo build --release -p os`. No primeiro boot ele é instalado em `/apps/meuapp.wasm`
   e aparece no launcher Apps. Dados do app (`fs=own`) ficam em `/data/meuapp/`.

Regras do jogo: entrada por `on_key(code, mods)`, `on_text`, `on_pointer`, `on_resize`,
`on_tick`, `on_close`; desenho por `Canvas` (`fill_rect`, `text`, `blit_rgba`, `png`);
arquivos por `File` (a raiz `/` é a pasta do app; `..` acima dela falha com `Errno::PERM`);
`render` só roda quando há motivo (entrada, tick, `request_redraw`), então um app parado não
gasta CPU. Um ponteiro inválido ou um laço infinito encerra **só** o seu app, com "O app
encerrou: <motivo>" na janela. `log!` escreve na serial (`[app <id>] ...`).

## 5. Problemas comuns

| Sintoma | Causa | Solução |
|---|---|---|
| `E0046: not all trait items implemented, missing: forward_overflowing` | Toolchain `nightly` sem data com `x86_64` < 0.15.5 | Use o pin do repo (`nightly-2026-10-05`) e `cargo update -p x86_64`. |
| `invalid rustc abi: 'x86-softfloat'` no `build.rs` do bootloader | `bootloader` < 0.11.17 com nightly novo | `cargo update -p bootloader` (0.11.17+). |
| UEFI: `panicked at load_kernel.rs:285 ... None` | RAM insuficiente: o BSS do kernel tem ~91 MiB e o bootloader o zera antes de entrar | Use `-m 192M` ou mais (os scripts usam 256M). |
| QEMU não sobe e não há `serial.log` | Caminho do socket do monitor > 107 bytes | Use `tools/qemu-headless.sh` (socket em `/tmp`) ou um `outdir` mais curto. |
| `cargo clippy` falha com erro de unwinding | O kernel `no_std` não compila no host | Use os aliases: `cargo lint-kernel`, `cargo lint-host`. |
| `cargo tree` / `cargo metadata` dá ICE | `-Z bindeps` | `cargo tree -p kernel --target x86_64-unknown-none`. |
| Tela preta no hardware real | Secure Boot ligado ou modo Legacy/CSM | Ver [`BOOT-USB.md`](BOOT-USB.md). |

## 6. Hardware real

Veja [`BOOT-USB.md`](BOOT-USB.md). Nenhum teste em hardware real foi feito durante
a auditoria; os números de desempenho e o comportamento de vídeo vêm do QEMU.
