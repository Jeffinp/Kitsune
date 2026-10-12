# Política de segurança

## Versões suportadas

Só a `master`. O Kitsune não tem releases versionadas; cada commit na `master`
passa por testes, lint e `cargo deny` (ver [`.github/workflows/ci.yml`](.github/workflows/ci.yml)).

## O que é e o que não é uma vulnerabilidade aqui

O Kitsune é um sistema operacional de estudo, **sem isolamento**: tudo roda em ring 0
num único espaço de endereçamento (veja [`docs/SECURITY-MODEL.md`](docs/SECURITY-MODEL.md)).
Isso não é bug, é uma decisão de projeto documentada. Interessam, em ordem:

1. **Entrada externa que derruba, trava ou corrompe o kernel**: frame de rede, resposta
   HTTP, HTML/CSS, imagem de disco OJFS, módulo `.wasm`.
2. **Fuga de um app WebAssembly** para fora da própria memória e das host functions.
3. **Qualquer caminho que contorne os limites** (corpo de 1 MiB, profundidade/nós/regras
   do HTML, *fuel* e memória do WASM).
4. Falhas de **supply chain**: dependência vulnerável ou de licença incompatível.

**Já conhecidos** (não precisam de relato, estão em [`docs/SECURITY-MODEL.md`](docs/SECURITY-MODEL.md#3-o-que-não-é-protegido)):
HTTPS sem revogação (CRL/OCSP) nem HSTS, ausência de ring 3, threads mortas que não são
reiniciadas nem liberam recursos, DHCP e DNS sem autenticação, imagem sem assinatura (Secure Boot desligado).

## Como relatar

Use **GitHub Private Vulnerability Reporting** (aba *Security* → *Report a vulnerability*
em <https://github.com/Jeffinp/Kitsune>) ou, se não estiver disponível, abra uma issue
sem detalhes de exploração pedindo um canal privado. Inclua:

- o commit testado e o modo (BIOS/UEFI, QEMU ou hardware);
- a entrada mínima que reproduz (um arquivo de `fuzz/regressions/` é o formato ideal);
- a saída da serial (`-serial file:serial.log`) e, se houver, a tela de erro.

Para achar bugs sozinho: `cd fuzz && cargo fuzz run web_parse` (veja
[`docs/TESTING.md`](docs/TESTING.md#2-fuzzing)). Todo crash vira um teste de
regressão em `kitsune_core` antes de ser corrigido.
