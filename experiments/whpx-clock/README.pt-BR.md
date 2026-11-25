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

Se o próprio RustEmuDroid instalou essas versões das ferramentas, use `--stock-sdk "$env:LOCALAPPDATA\RustEmuDroid\runtime"`. Após as três verificações, abra o RustEmuDroid v0.22.0 ou superior e escolha a pasta de destino em **Configurações → Runtime, imagem e perfil → Cache de memória Vulkan**. Os jogos ficam no perfil Android que você já usava. [Tela em português](../../docs/screenshots/pt-br/vulkan-cache-settings.png)

O SuperTuxKart escreve dados dos objetos em um buffer que a CPU pode acessar. Na RTX 5070 Ti, medimos o efeito de usar memória com cache para essas escritas. Para experimentar, use a variante `cache` da receita de compilação e aponte `--sdk` para esse motor. O parâmetro `--fix vulkan-cache` deixa o ajuste de relógio desligado nos dois modos. [Medições de memória](../../benchmarks/stk-1.5/windows-vulkan-mapped-writes.md)

```powershell
$opcoes = @('--app', $app, '--sdk', $sdk, '--profile', $perfil, '--fix', 'vulkan-cache')
python $teste check @opcoes
python $teste on @opcoes --hz 90
python $teste off @opcoes --hz 90
```
