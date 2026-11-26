# Testar o relógio virtual e o cache Vulkan na janela do RustEmuDroid

Este teste usa a janela normal do RustEmuDroid e uma compilação experimental do motor Android para Windows x64 com WHPX. Com a RTX 5070 Ti, o SuperTuxKart passou de 85,65 para 87,46 FPS em quatro corridas. A versão v0.21.1 disponível nos Downloads usa o motor habitual. [Resultados do teste](../../benchmarks/stk-1.5/windows-whpx-clock-kart.md)

## Preparar o ambiente

Use Python 3.11 ou superior, a versão Windows do RustEmuDroid e a imagem Google Play x86-64 do Android 17, revisão 7. A receita de compilação cria uma pasta com `sdk` e o recibo `staging.json` ao lado dela. O recibo identifica o QEMU que será executado. [Preparação do motor](README.md) · [Downloads do RustEmuDroid](https://github.com/felipemarques/RustEmuDroid/releases)

Abra o PowerShell na raiz do repositório. Ajuste os caminhos para o local do seu executável, do motor preparado e da imagem já instalada. Se estiver criando um perfil de testes, o comando `init` abaixo registra a imagem e cria o diretório do Android. Os jogos instalados nesse perfil ficam disponíveis nas sessões seguintes.

```powershell
$app = 'D:\Apps\RustEmuDroid\rustemudroid.exe'
$sdk = 'D:\Testes\motor-whpx\sdk'
$imagem = "$env:LOCALAPPDATA\Android\Sdk\system-images\android-37.0\google_apis_playstore\x86_64"
$perfil = 'D:\Testes\android17-jogos'
& $app --sdk $sdk --state-dir $perfil init --system-dir $imagem
```

Se já tiver um perfil de testes com essa imagem e quatro CPUs virtuais, aponte `$perfil` para a pasta que contém `avd\RustEmuDroid.ini` e pule o `init`.

## Abrir e comparar

O modo `check` confere o motor, a imagem e o perfil em disco. Pode usá-lo com o emulador aberto. Para os modos `on` e `off`, feche a janela atual do RustEmuDroid. Execute uma linha por vez. O comando aguarda o fechamento da janela e confere o registro daquele boot.

```powershell
$teste = '.\experiments\whpx-clock\run.py'
$opcoes = @('--app', $app, '--sdk', $sdk, '--profile', $perfil)
python $teste check @opcoes
python $teste on @opcoes
python $teste off @opcoes
```

`on` sincroniza o relógio das quatro CPUs virtuais e `off` inicia o mesmo motor com a opção desligada. Para reproduzir uma corrida em 90 Hz, acrescente `--hz 90` aos dois comandos. Para escolher a GPU, use o nome mostrado pelo RustEmuDroid, por exemplo `--gpu-device "NVIDIA GeForce RTX 5070 Ti"`. Compare o mesmo replay e a mesma qualidade gráfica. Executar outro jogo durante a medição também consome CPU e GPU.

O teste cria preferências temporárias para a janela. O Android grava os jogos, partidas e instalações no perfil escolhido. Ao sair normalmente, o terminal mostra o caminho do log com os marcadores conferidos. Abra o atalho habitual para voltar ao motor instalado.

## Testar o cache de memória Vulkan

Com o ZIP dos módulos de referência em mãos, você pode preparar o motor no Windows usando Python 3.11 ou superior. Na raiz do projeto, informe o ZIP, a pasta das ferramentas oficiais (Android Emulator 37.3.10 e Platform Tools 37.0.0) e uma nova pasta de destino.

```powershell
python .\experiments\whpx-clock\stage.py stage --overlay-zip 'D:\Downloads\whpx-cache-windows-x86_64.zip' --stock-sdk "$env:LOCALAPPDATA\Android\Sdk" --out 'D:\Android\CacheEngine' --smoke
```

A pasta preparada herda as permissões da pasta onde ela fica, então um motor preparado num terminal com privilégios de administrador continua legível pelo RustEmuDroid aberto normalmente. Pastas preparadas antes dessa correção, com Python 3.12.4 ou superior e privilégios de administrador, ficaram com permissões privadas; corrija com `icacls <pasta> /reset /T` num terminal de administrador.

Se o próprio RustEmuDroid instalou essas versões das ferramentas, use `--stock-sdk "$env:LOCALAPPDATA\RustEmuDroid\runtime"`. Após as três verificações, abra o RustEmuDroid v0.22.0 ou superior e escolha a pasta de destino em **Configurações → Runtime, imagem e perfil → Cache de memória Vulkan**. Os jogos ficam no perfil Android que você já usava. [Tela em português](../../docs/screenshots/pt-br/vulkan-cache-settings.png)

O ZIP do motor Windows recompilado com FFmpeg 4.4.2 e libvpx 1.12.0 tem SHA-256 `a67d12ec5abbb220cce81ecf623174d278fab08dd5d0a08441be51c642395146`. Para conferir o download e seus módulos antes de escolher o destino, troque o comando `stage` após `stage.py` por `check` e remova `--out` e `--smoke`. Ao concluir, ele mostra o hash do ZIP e os **20** caminhos de instalação esperados. A preparação grava o hash em `staging.json`. Para voltar ao motor anterior, escolha **Voltar ao motor padrão** no mesmo perfil, salve e reinicie o Android.

O SuperTuxKart escreve dados dos objetos em um buffer que a CPU pode acessar. Na RTX 5070 Ti, medimos o efeito de usar memória com cache para essas escritas. Para experimentar, use a variante `cache` da receita de compilação e aponte `--sdk` para esse motor. O parâmetro `--fix vulkan-cache` deixa o ajuste de relógio desligado nos dois modos. [Medições de memória](../../benchmarks/stk-1.5/windows-vulkan-mapped-writes.md)

```powershell
$opcoes = @('--app', $app, '--sdk', $sdk, '--profile', $perfil, '--fix', 'vulkan-cache')
python $teste check @opcoes
python $teste on @opcoes --hz 90
python $teste off @opcoes --hz 90
```

## Motor de desempenho

A variante `performance` recompila o núcleo QEMU e o renderizador gfxstream com as mudanças usadas nas medições de 26 de novembro em Mailbox sem limite. Na RTX 5070 Ti, a receita marcou 356 FPS no modo de energia Equilibrado do Windows e 375 em Melhor desempenho, contra 164–176 com o motor original. [Medições, receita e limites](../../docs/performance-history.pt-BR.md#motor-de-desempenho-em-mailbox-sem-limite)

As fontes são o commit `538bcdea5479b3857cd8885cbb9306bfec9c7247` do `external/qemu` com três patches em ordem (`whpx-tsc-sync.patch`, `gfxstream-cached-host-qa.patch`, `performance-engine-qemu.patch`) e o commit `501867e6a3b1a3be7b935cc977d291150846edab` do `hardware/google/gfxstream` com `performance-engine-gfxstream.patch`, todos em `benchmarks/stk-1.5/`. O arquivo [performance-engine.json](performance-engine.json) registra os hashes dos patches, os arquivos resultantes e as variáveis de cada opção. Cada opção fica desligada enquanto a sua variável não estiver definida, exceto a P0, que grava na hora as linhas de confirmação do QEMU (sem ela, o Windows só gravava algumas linhas no log ao fechar o motor). As fontes publicadas geram a compilação QEMU `fc990b45…b94f` e gfxstream `7d01cde7…85fe` do PC de testes, que inclui a opção R, `RUSTEMUDROID_QA_WHPX_APIC=hv`: o hipervisor do Windows passa a emular o APIC local de cada vCPU (xAPIC) no lugar do APIC em modo usuário do QEMU. Numa série durante o dia, R elevou o SuperTuxKart de 345,2 para 380,1 FPS; foi testada só com boots a frio de uma imagem num PC AMD. Se o host não oferecer o recurso, o motor grava `RustEmuDroid QA: WHPX hypervisor local APIC unavailable (...)` e continua com o APIC em modo usuário. No cliente v0.24.0, R é a capacidade `whpx_apic_hv` (acrescente `--capability whpx_apic_hv` ao `write`). A compilação do kit da v0.23.0 (QEMU `1c7a05e3…a0c6`) é a mesma fonte sem R. As medições da noite usaram a compilação anterior, QEMU `a55d48c6…37f5` e gfxstream `d92f6060…60d0`, que é a mesma fonte sem P0, L2 (`RUSTEMUDROID_QA_RAM_SECTION`), P (`RUSTEMUDROID_QA_ASG_SPIN_RENDERING`), Q (`RUSTEMUDROID_QA_MMIO_FASTPATH`), as estatísticas A3 e R. P e Q não foram validadas e o cliente não as oferece. Os comandos abaixo usam as variáveis `$Stage`, `$Source`, `$Build`, `$Stock` e `$Perf` do [guia em inglês](README.md#4-build-the-performance-engine):

```powershell
python -B $Stage build --variant performance --source $Source --out $Build
python -B $Stage stage --stock-sdk $Stock --artifacts $Build --out $Perf --smoke
python -B .\experiments\whpx-clock\engine_manifest.py evidence --engine $Perf
python -B .\experiments\whpx-clock\engine_manifest.py write --engine $Perf --capability tsc_sync --capability asg_spin
```

O `write` grava `rustemudroid-engine.json` ao lado de `sdk`, com os SHA-256 do launcher, do QEMU e do gfxstream. Ele só aceita uma capacidade quando encontra a variável ou a mensagem de confirmação correspondente no binário. Repita `--capability` para cada opção que o motor oferece (lista completa no guia em inglês). Depois, no RustEmuDroid v0.23.0, abra **Configurações → Runtime, imagem e perfil → Motor de desempenho (experimental)**, escolha a pasta e reinicie o Android.

Licenças: o QEMU é GPL-2.0, e os patches do QEMU seguem a mesma licença. A fonte correspondente de um QEMU recompilado é o commit fixado acima mais os três patches, nessa ordem. Quem distribuir o binário precisa oferecer essa fonte nos termos da GPL. O gfxstream e o seu patch são Apache-2.0. O repositório publica só fontes e patches, sem binários do motor.
