# SDK de apps: seu primeiro app em dez minutos

Um app do Kitsune é **um arquivo `.wasm`**: código WebAssembly mais um manifesto (e, se quiser, um
ícone). Este texto leva do zero a um app rodando no Kitsune **sem recompilar o sistema**. O contrato
completo (ABI, permissões, limites) está em [`design/apps.md`](design/apps.md); aqui fica o caminho
mais curto.

## O que você precisa

* Rust com o alvo WebAssembly: `rustup target add wasm32-unknown-unknown`.
* Este repositório clonado (o SDK é a crate `wasm-apps/sdk`; ele é `no_std` e não tem dependências).
* Um Kitsune rodando (QEMU ou uma máquina) com rede, para baixar o arquivo. Se preferir não usar a
  rede, o passo 6 mostra como embutir o app na imagem.

## 1. Crie a crate

Fora do workspace do repositório (cada app tem o seu, como os de `wasm-apps/`):

```sh
mkdir -p ~/meuapp/src && cd ~/meuapp
```

`Cargo.toml` (troque o caminho do SDK pelo do seu clone):

```toml
[package]
name = "meuapp"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
kitsune-sdk = { path = "/caminho/para/Kitsune/wasm-apps/sdk" }

[profile.release]
opt-level = "s"
lto = true
panic = "abort"

[workspace]
```

## 2. Escreva o app

`src/lib.rs`: um contador que cresce a cada tecla ou clique e guarda o valor em arquivo.

```rust
#![no_std]
use kitsune_sdk::*;

manifest!("id=meuapp\nname=Meu App\nname.pt=Meu App\nname.en=My App\nversion=1.0.0\nfs=own\nwin_w=360\nwin_h=200\n");

struct Contador {
    n: u32,
}

impl App for Contador {
    fn new() -> Self {
        // `fs=own`: a raiz `/` é a pasta de dados do app (`/data/meuapp`).
        let mut n = 0;
        if let Ok(mut f) = File::open("/n", O_READ) {
            let mut b = [0u8; 4];
            if f.read_full(&mut b).unwrap_or(0) == 4 {
                n = u32::from_le_bytes(b);
            }
        }
        Contador { n }
    }

    fn on_key(&mut self, _code: i32, _mods: i32) {
        self.n += 1;
        request_redraw();
    }

    fn on_pointer(&mut self, _x: i32, _y: i32, buttons: i32) {
        if buttons & BTN_LEFT != 0 {
            self.n += 1;
            request_redraw();
        }
    }

    fn on_close(&mut self) {
        if let Ok(mut f) = File::open("/n", O_WRITE | O_CREATE | O_TRUNC) {
            let _ = f.write_all(&self.n.to_le_bytes());
        }
    }

    fn render(&mut self, c: &mut Canvas) {
        c.clear(0x10141F);
        c.text(16, 16, tr("Toques", "Taps"), 0x9AA6BD, 2);
        let mut b = StrBuf::<16>::new();
        let _ = core::fmt::Write::write_fmt(&mut b, format_args!("{}", self.n));
        c.text(16, 60, b.as_str(), 0xFFFFFF, 6);
    }
}

export_app!(Contador);
```

Só `new` e `render` são obrigatórios; os outros eventos (`on_key`, `on_text`, `on_pointer`, `on_resize`,
`on_tick`, `on_close`) têm corpo vazio por padrão. O `render` só roda quando há motivo (entrada, tick,
`request_redraw`): um app parado não gasta CPU.

## 3. Compile

```sh
cargo build --release --target wasm32-unknown-unknown
ls -l target/wasm32-unknown-unknown/release/meuapp.wasm   # alguns KiB
```

(Fora do clone, o `cargo` usa o seu toolchain padrão, que precisa do alvo do primeiro item. Se preferir o
toolchain que o repositório fixa, que já traz o alvo: `cargo +nightly-2026-10-05 build --release
--target wasm32-unknown-unknown`.)

## 4. Leve o arquivo ao Kitsune

No computador onde você compilou, sirva a pasta:

```sh
cd target/wasm32-unknown-unknown/release && python3 -m http.server 8000
```

No **Terminal** do Kitsune (no QEMU com rede de usuário, o computador é `10.0.2.2`; numa rede real,
o IP dele):

```sh
wget http://10.0.2.2:8000/meuapp.wasm
```

O arquivo cai na pasta atual do Terminal (a sua pasta de usuário, `/home/<nome>`).

## 5. Abra e use

No **Arquivos**, vá para a sua pasta, selecione `meuapp.wasm` e aperte **Enter**: o sistema valida o
pacote, instala em `/apps/meuapp.wasm` e executa. Dali em diante o app aparece no launcher **Apps**. Os
dados dele ficam em `/data/meuapp/`. Para ver o que o manifesto pede, use **Informações** (botão
direito) no `.wasm`. Para remover, o lugar **Apps** do Arquivos tem *Remover* (os dados ficam).

Uma recusa na instalação diz o motivo (manifesto com chave desconhecida, pedido acima do teto,
id repetido, ícone inválido): o instalador **recusa**, nunca ajusta em silêncio.

## 6. Embutir na imagem do sistema (opcional)

Para que o app venha instalado em todo disco novo: coloque a crate em `wasm-apps/meuapp/`, acrescente
`"meuapp"` em `BUNDLED_APPS` de `kernel/build.rs` e rode `cargo build --release -p os`. O primeiro boot
o instala (uma vez: se o usuário remover, ele não volta).

## Manifesto em uma tela

Uma chave por linha (`chave=valor`). Obrigatórias: `id`, `name`, `version`.

| Chave | Valores | Padrão |
|---|---|---|
| `fs` | `none` (sem arquivos), `own` (`/` = `/data/<id>`), `home` (`/` = a pasta do usuário que entrou) | `none` |
| `net` / `net_hosts` | `none`, `http` (só GET); lista de destinos permitidos | `none` |
| `clipboard` | `none`, `rw` | `none` |
| `mem_mib`, `disk_kib`, `max_fds` | tetos 24, 4096, 32 | 8, 256, 16 |
| `fuel_frame` | instruções por chamada (até 20 000 000) | 4 000 000 |
| `tick_ms` | período de `on_tick` (0 = sem ticks) | 0 |
| `win_w`, `win_h`, `win_min_w`, `win_min_h`, `resizable` | tamanho da área de conteúdo | 640 x 400 |
| `name.pt`, `name.en` | nome por idioma (até 24 caracteres) | `name` |

## Limites que importam

* Um app que estoura o combustível de um quadro ou usa um ponteiro inválido **encerra sozinho**, com a
  mensagem na janela; nunca derruba o sistema nem outro app.
* A raiz de arquivos é fixa: `..` acima dela falha com `Errno::PERM`. Não há acesso a `/apps`, a
  `/data/<outro>` nem ao sistema.
* Os apps rodam como o **usuário que entrou** só no sentido do diretório (`fs=home`): hoje todo código
  roda no anel 0 e o isolamento é o do WebAssembly (veja [`SECURITY-MODEL.md`](SECURITY-MODEL.md)).
* `log!("...")` escreve na serial (`[app meuapp] ...`), útil no QEMU (`-serial stdio`).

## Próximos passos

Os exemplos de `wasm-apps/examples/` (`hello`, `plasma`, `nettest`) e os apps que acompanham o sistema
(`clock`, `notes`, `paint`, `snake`) mostram desenho, arquivos e rede. Programas em C, Rust ou Go com
WASI (sem este SDK) ainda não rodam por inteiro: ver [`design/compatibilidade.md`](design/compatibilidade.md),
passo A.3.
