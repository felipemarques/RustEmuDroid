# RustEmuDroid

[English](README.md) · [Español](README.es-ES.md) · [Downloads para Windows e Linux](https://github.com/felipemarques/RustEmuDroid/releases/latest) · [Resultados de desempenho](benchmarks/stk-1.5/README.md)

O RustEmuDroid executa jogos e aplicativos Android em uma única janela no Windows e no Linux x86-64. O cliente em Rust inicia a máquina virtual, exibe a tela do Android e controla a entrada e a janela. Dentro da VM, a Home abre diretamente na biblioteca de jogos instalados. O painel e a Home são livres de anúncios. O idioma padrão é português brasileiro. Nas configurações, escolha inglês dos Estados Unidos ou espanhol da Espanha e salve para mudar o painel e a Home do Android juntos. Cada jogo ou aplicativo instalado controla o próprio idioma.

![Janela do RustEmuDroid com a biblioteca de jogos do Android](docs/screenshots/pt-br/games.png)

*Os jogos da captura foram instalados no perfil de teste. Um perfil novo começa com os aplicativos fornecidos pela imagem Android.*

## Executar uma versão compilada

1. Abra a [versão mais recente no GitHub](https://github.com/felipemarques/RustEmuDroid/releases/latest) e localize **Assets**.
2. No Windows, extraia `RustEmuDroid-vX.Y.Z-windows-x86_64.zip` em uma pasta com permissão de escrita. No Linux, extraia `RustEmuDroid-vX.Y.Z-linux-x86_64.tar.gz` com `tar -xzf` para preservar a permissão de execução. O nome do arquivo traz a versão no lugar de `X.Y.Z`.
3. No Windows, abra `rustemudroid.exe`. No Linux, execute `./rustemudroid` na pasta extraída. Em um PC sem runtime, selecione **Instalar runtime ou imagem Android**.

| Ação | Windows PowerShell | Terminal Linux |
|---|---|---|
| Verificar o motor e a virtualização | `.\rustemudroid.exe doctor` | `./rustemudroid doctor` |
| Abrir o painel | `.\rustemudroid.exe ui` | `./rustemudroid ui` |

Para baixar uma compilação de desenvolvimento, entre no GitHub, abra uma [execução bem-sucedida de Desktop builds](https://github.com/felipemarques/RustEmuDroid/actions/workflows/build.yml) e baixe o pacote em **Artifacts**.

O executável inclui o APK da Home do RustEmuDroid. O motor e a imagem Android são fornecidos separadamente. Compilar o projeto também exige Rust e Cargo.

## Preparar o ambiente Android

| Requisito | Windows x86-64 | Linux x86-64 |
|---|---|---|
| Virtualização | Habilite Intel VT-x ou AMD-V no firmware e **Plataforma do Hipervisor do Windows** nos Recursos do Windows. | Habilite a virtualização e permita que seu usuário leia e escreva em `/dev/kvm`. Confira com `test -r /dev/kvm && test -w /dev/kvm`. |
| Vídeo e desktop | Instale o driver da placa de vídeo para usar aceleração da GPU. | Use uma sessão gráfica X11 ou Wayland com OpenGL. O seletor de APK depende do portal de arquivos do desktop. |
| Ambiente Android | Use o assistente de instalação, o Kemu já instalado ou um SDK compatível. | Use o assistente de instalação ou um runtime x86-64 compatível. |

### Primeira instalação pelo RustEmuDroid

1. Escolha **Original** para Android 15 AOSP ou **Google Play** para Android 17 com Play Store. Clique em **Consultar download** para ver a pasta, o tamanho máximo e os termos do Google.
2. Se concordar com os termos, marque a caixa e clique em **Baixar e instalar**. O RustEmuDroid baixa o emulador, o ADB e a imagem diretamente do Google e confere o tamanho e o checksum de cada arquivo. Reserve espaço também para descompactá-los.
3. Clique em **Usar e iniciar este perfil**. Para adicionar a outra imagem depois, abra o mesmo assistente pela engrenagem. O instalador reutiliza os componentes gerenciados já presentes. Cada perfil tem seus próprios dados de aplicativos.

![Assistente de instalação em português com o tamanho e os termos do download](docs/screenshots/pt-br/setup.png)

O assistente funciona sem uma instalação prévia do Android SDK, Android Studio, Java ou `sdkmanager`. No Windows, os componentes ficam em `%LOCALAPPDATA%\RustEmuDroid\runtime`. No Linux, ficam em `$XDG_DATA_HOME/RustEmuDroid/runtime` ou, por padrão, `~/.local/share/RustEmuDroid/runtime`.

No terminal, execute `runtime-setup --image original` ou `runtime-setup --image google-play` como argumento do executável para consultar os termos e o tamanho. Para iniciar o download após a leitura, repita o comando com `--accept-license`. Para reaproveitar um aceite anterior idêntico, use `--licenses-from <pasta-de-licenças>`.

**Windows com Kemu.** Com o Kemu na pasta padrão, execute `.\rustemudroid.exe ui` para abrir o perfil Original. Para uma instalação em outra pasta, defina `RUSTEMUDROID_KEMU_ROOT` com esse caminho. Esse modo usa o ADB, a imagem e o motor fornecidos pelo Kemu. Selecione GPU do computador ou Automática.

**Runtime portátil no Windows ou Linux.** Dentro da pasta do executável, crie `runtime/`. Coloque nela os pacotes completos `emulator/` e `platform-tools/` do seu sistema e a árvore `system-images/` com a imagem Android. Execute `ui` a partir da pasta extraída. O launcher reconhece essa estrutura mesmo sem Android Studio ou variáveis do SDK. Preserve as bibliotecas e todos os arquivos da imagem. A opção `--sdk` continua disponível para selecionar outra instalação.

**Instalação pelo SDK.**

Com `sdkmanager` disponível no PATH, este comando instala as ferramentas e a imagem opcional do Google Play. Aceite as licenças do SDK quando forem apresentadas.

```sh
sdkmanager --install "emulator" "platform-tools" "system-images;android-37.0;google_apis_playstore;x86_64"
```

Outra imagem compatível é `system-images;android-35;google_apis;x86_64`. Consulte os pacotes com `sdkmanager --list`. Para localizar uma instalação do SDK, o cliente também aceita `ANDROID_HOME`, `ANDROID_SDK_ROOT` e a pasta padrão do sistema. [SDK Manager](https://developer.android.com/tools/sdkmanager) · [Aceleração do Android Emulator](https://developer.android.com/studio/run/emulator-acceleration)

No Ubuntu 24.04 com WSL2, a versão Linux iniciou o Android 17 com Google Play usando KVM. No WSLg, o BombSquad apresentou 60 quadros/s em uma rodada de Death Match com OpenGL ES nativo na Radeon 780M via Mesa D3D12. O Vulkan do Android usou a CPU nesse computador. [Configuração, seleção do driver e medições, em inglês](docs/linux-runtime.md)

## Escolher um perfil

Abra a engrenagem na barra de título. Cada perfil tem seu próprio disco gravável, aplicativos, contas e progresso salvo. **Original** é o perfil padrão daquele diretório de trabalho. A versão do Android depende da imagem escolhida.

| Configuração | Motor e primeiro uso |
|---|---|
| Original com Kemu | No Windows, o RustEmuDroid pode usar a imagem Android 16 / API 36 e o motor QEMU/gfxstream da instalação do Kemu nos modos GPU do computador e Automática. A configuração testada usa ANGLE/Vulkan. Configure `RUSTEMUDROID_KEMU_ROOT` para instalações fora da pasta padrão. |
| Original com SDK ou runtime portátil | No primeiro uso, a busca verifica Google Play API 37.0, Google APIs 36, Google APIs 35 e depois a imagem AOSP `default` x86-64 mais recente disponível. Use `init --image` para selecionar um pacote específico. |
| Android 17 com Google Play | Instale a imagem Google Play API 37.0, selecione **Android 17** na engrenagem e salve. A imagem do SDK fornece a Play Store e o Google Play Services. O cartão indica quando a imagem está ausente. |

A configuração de jogos solicita quatro vCPUs, 8 GiB de RAM para o Android e 1600 × 900 a 60 Hz. O perfil genérico do SDK solicita quatro vCPUs, 3 GiB e 1080 × 1920. O motor do Kemu no Windows usa um disco gravável separado na pasta `gaming/`.

Para testar uma tela de alta frequência, abra **Configurações → Gráficos → Taxa de atualização do Android**, escolha um valor até a taxa atual do monitor e salve. O Android reinicia. Pela linha de comando, use `boot --refresh-rate 90`. **Usar a taxa do perfil** volta à frequência original do perfil. Em versões compatíveis do Android, escolher mais de 60 Hz também solicita essa taxa para os jogos durante a sessão. Cada jogo pode ter um limite menor. [Medições do SuperTuxKart](docs/game-testing.md#managed-windows-runtime-and-android-refresh-rate)

![Configurações dos perfis, da renderização, da placa Vulkan e da webcam](docs/screenshots/pt-br/settings.png)

## Usar os controles

| Ação | Como usar |
|---|---|
| Encontrar e instalar apps | Busque nas abas **Jogos** e **Aplicativos**. Pressione e segure um cartão para mudar sua categoria. O botão de instalação da barra direita aceita um APK e atualiza a biblioteca. |
| Rolar a biblioteca do Android | Com o ponteiro sobre os cartões em **Jogos** ou **Aplicativos**, gire a roda do mouse ou deslize dois dedos na vertical no touchpad. |
| Navegar no Android | O botão de quatro quadrados abre Aplicativos. A barra superior também traz **Voltar**, **Início** e **Aplicativos recentes**. Início retorna à aba Jogos. Clique para tocar e arraste para deslizar. |
| Mapear teclado ou controle | Abra o jogo e clique no ícone de teclado. Em **Botão**, clique no ponto de toque e pressione a tecla ou botão físico. Para usar WASD ou o analógico esquerdo, escolha **Direcional**, clique no centro do joystick virtual e ajuste o raio. Clique com o botão direito para remover uma marca. **Salvar** guarda as posições relativas para aquele pacote Android. No Windows, os controles usam XInput. |
| Ajustar mídia | A barra direita permite subir ou baixar o volume de mídia, silenciar, reativar e girar a tela. Segure um botão de volume para alterar vários níveis. |
| Usar uma webcam | Na engrenagem, selecione a webcam frontal ou traseira do computador. O índice 0 representa a primeira câmera. Salve e reinicie o Android para aplicar a mudança. Conceda a permissão dentro do aplicativo Android quando necessário. Também há uma câmera frontal simulada. |
| Tela cheia e versão | Pressione **F11**. Aproxime o ponteiro da borda superior ou direita para revelar os controles em tela cheia. A versão aparece na barra de título e na janela **?**. |

![Janela de ajuda com a marca e a versão do RustEmuDroid](docs/screenshots/pt-br/about.png)

**Encerrar Android** desliga a VM e volta ao painel. Ao fechar a janela pelo **X** ou pelo atalho do sistema, o painel exibe **Desligando Android…**, continua respondendo e fecha automaticamente quando a VM termina. Isso também funciona durante a inicialização. Com o painel ocioso, a janela fecha imediatamente.

![Janela aguardando o encerramento do Android](docs/screenshots/pt-br/shutdown.png)

Ao encerrar o Android, a tela da sessão permite iniciar o mesmo perfil novamente.

![Tela da sessão após encerrar o Android](docs/screenshots/pt-br/idle.png)

## Abrir uma instância instalada do BlueStacks no Windows

No Windows, escolha **Abrir BlueStacks → Usar nesta janela** para ver a instância instalada e seus jogos no RustEmuDroid. Mantenha a janela original do player restaurada. O Windows compõe a imagem do Android dentro da janela Rust. Ao desconectar, a instância e o jogo continuam abertos. Se o Android do RustEmuDroid estiver em execução, ele será encerrado antes da conexão.

![Escolha a instância e Usar nesta janela para abrir a prévia do jogo](docs/screenshots/pt-br/bluestacks.png)

Para usar toque, arraste, rolagem, teclado e os botões de navegação Android, ative **Android Debug Bridge (ADB)** nas configurações avançadas do BlueStacks e reconecte. Clique em **Girar tela** na barra da prévia para alternar entre retrato e paisagem nos aplicativos que aceitam rotação. O RustEmuDroid usa um cliente ADB moderno do runtime gerenciado ou das Android Platform Tools. Com o ADB desativado, o modo permite apenas visualizar. Para acompanhar os quadros na prévia, escolha **Configurações → Contador de FPS → Superfície do jogo no Android**. A sessão Android nativa também oferece gravação e o contador do painel Rust.

Na prévia do BlueStacks, abra um jogo e clique no ícone de teclado e escolha **Editor de controles**, ou pressione **Ctrl+Shift+A**. Posicione os controles sobre a captura do jogo. O Android continua rodando enquanto você edita. Salve para voltar à imagem ao vivo. **Ctrl+Shift+P** alterna entre os perfis salvos para aquele jogo e aquela instância do BlueStacks. Segure o direcional e um botão de ação ao mesmo tempo para enviar dois toques. O editor mostra as marcações das teclas. Na sessão Android nativa, também é possível ver as marcações durante o jogo e mapear o sensor de inclinação.

![Mapeamento do SuperTuxKart na prévia do BlueStacks](docs/screenshots/pt-br/bluestacks-editor.png)

[Menu rápido de controles](docs/screenshots/pt-br/bluestacks-quick.png)

**Posso usar os jogos do meu backup do BlueStacks?** No PC de testes, restauramos uma cópia do disco de dados em outra instância Pie64 da mesma versão do BlueStacks. Ela iniciou o Android 9 e listou os quatro jogos verificados. Selecione a instância restaurada em **Abrir BlueStacks → Usar nesta janela**. [Teste de restauração, em inglês](docs/bluestacks-lab.md#booting-an-archived-data-disk-with-the-installed-bluestacks-engine)

**Posso iniciar a cópia da imagem sem instalar o BlueStacks?** O híbrido experimental do Pie64 chegou ao serviço de sistema do Android (`system_server`) e parou antes da tela inicial. O boot de 29,5 segundos foi medido com a imagem de referência do Android 9. [Resultados do Pie64, em inglês](docs/bluestacks-lab.md#r2-fstab-compatibility-test)

**Se você baixou o ZIP da release**, use **Extrair tudo** e abra no Explorador de Arquivos a pasta em que aparece `rustemudroid.exe`. Clique na barra de endereço, digite `powershell` e pressione **Enter**. Nessa janela, indique qual executável usar.

```powershell
$emu = '.\rustemudroid.exe'
```

**Se você clonou o repositório**, abra no Explorador a pasta que contém `Cargo.toml` e digite `powershell` na barra de endereço. O atalho `Abrir RustEmuDroid.cmd` compila o executável neste caminho.

```powershell
$emu = '.\target\integrado\release\rustemudroid.exe'
```

Se for a primeira compilação, execute `cargo build --locked --release --features ui --target-dir target/integrado` nessa pasta. Se você já compilou com o comando padrão do Cargo, use `$emu = '.\target\release\rustemudroid.exe'`.

**Confira o caminho e liste as instâncias.** Continue na mesma janela do PowerShell. `Test-Path` deve retornar `True`. Se retornar `False` ou o PowerShell disser que o comando não foi reconhecido, confira a pasta aberta e o caminho escolhido acima. O prefixo `.\` significa “a partir da pasta atual”.

```powershell
Test-Path -LiteralPath $emu -PathType Leaf
& $emu bluestacks list
```

Exemplo de saída de `list` neste computador

```text
Pie64    BlueStacks App Player
```

A primeira coluna é o ID da instância. Para abrir a tela inicial do BlueStacks, use o ID que apareceu na sua lista no lugar de `Pie64`.

```powershell
& $emu bluestacks open --instance Pie64
```

Para abrir diretamente o SuperTuxKart, use o comando abaixo se o jogo estiver instalado nessa instância. `--package` recebe o identificador Android do jogo.

```powershell
& $emu bluestacks open --instance Pie64 --package org.supertuxkart.stk
```

Para exibir o jogo dentro do RustEmuDroid, execute

```powershell
& $emu bluestacks mirror --instance Pie64 --package org.supertuxkart.stk
```

Se a lista estiver vazia ou aparecer um erro de instalação, abra o Gerenciador de Multi-instâncias do BlueStacks 5, confirme que há uma instância instalada e repita `list`. Pela interface do RustEmuDroid, você também pode escolher **Abrir BlueStacks** na tela de sessão ou nas configurações.

A partir da v0.5.0, os dois pacotes incluem um teste de armazenamento somente leitura em `tools/`. Desligue a instância BlueStacks Pie64 e coloque cópias de `fastboot.vdi` e `Data_orig.vhdx` em uma pasta `Pie64` ao lado de `tools/`. Execute a partir da pasta extraída ou da raiz do código-fonte. Escolha uma pasta nova para os resultados a cada execução.

No Windows com Ubuntu 24.04 no WSL, execute

```powershell
powershell -ExecutionPolicy Bypass -File .\tools\Test-BlueStacksData.ps1 -Images .\Pie64 -Output .\data-check
Get-Content .\data-check\result.json
```

No Linux ou WSL, execute

```sh
python3 -B tools/bluestacks_data_probe.py --images ./Pie64 --output ./data-check
cat ./data-check/result.json
```

O teste aceita apenas o par exato de imagens Pie64 validado e verifica automaticamente os dois hashes SHA-256. Se as cópias já tiverem um `manifest.json`, acrescente `-VerifyHashes` ao comando do teste no Windows para conferir também os hashes desse manifesto.

O valor `true` em `factory_ext4_mounted_read_only` no `result.json` indica que o ambiente de diagnóstico montou o sistema de arquivos ext4 de dados de fábrica em modo somente leitura. O arquivo `tools/README.md` do pacote lista os requisitos.

Em um teste separado com `-SystemMount` no código-fonte, a cópia do sistema Android foi montada e a inicialização do BlueStacks parou em `android_id is empty`. Para jogar nessa instância, use **Abrir BlueStacks**. [Notas sobre a montagem do sistema, em inglês](docs/bluestacks-lab.md#opt-in-system-mount)

O caranguejo se movimenta durante a inicialização e o desligamento, na tela Sobre e quando a busca da Home do Android não encontra itens. Durante a partida, a animação fica desativada.

![Prévia da animação do caranguejo na interface](docs/mascot-preview.png)

Clique em **? → Atalhos** na barra de título para ver as teclas disponíveis, o que cada uma faz e quando usá-la.

![Aba Atalhos na ajuda](docs/screenshots/pt-br/shortcuts.png)

## Capturar uma jogada

| Ação | Atalho | Arquivo |
|---|---|---|
| Tirar print | **Ctrl+Shift+S** | PNG na resolução da tela do Android |
| Iniciar ou parar o vídeo | **Ctrl+Shift+R** | MP4 H.264 sem áudio, com alvo de 30 FPS |
| Gravar um momento | **Ctrl+Shift+J** | Os próximos três segundos em JPEG XL animado a 10 FPS |
| Salvar um lance recente | **Ctrl+Shift+H** | Os três segundos anteriores em JPEG XL animado a 10 FPS, com o replay instantâneo ligado |
| Gravar um GIF | Botão GIF na barra lateral | Os próximos três segundos em GIF em loop a 10 FPS |

Para guardar um gol ou uma ultrapassagem que acabou de acontecer, abra o relógio na barra lateral e ative o replay instantâneo **antes** de jogar. Quando estiver pronto, aperte **Ctrl+Shift+H** depois do lance ou salve pelo próprio menu. O replay começa desligado a cada sessão do Android. Desligá-lo descarta o trecho que ainda estava na memória.

![Menu de replay instantâneo no aplicativo em português](docs/screenshots/pt-br/replay.png)

O botão de pasta na barra lateral abre `captures/`, ao lado de `launcher.json`. O Rust grava o PNG diretamente. Para MP4, instale FFmpeg com `libx264`. Para JPEG XL animado, use o codificador `libjxl_anim`. Se o FFmpeg estiver fora do PATH, informe o caminho do executável em `RUSTEMUDROID_FFMPEG`. As animações preservam a proporção da imagem e cabem em 720 × 720. Ao fechar a janela, o cliente aguarda as exportações em andamento.

## Criar um esquema de controle

Abra o jogo e clique no ícone de teclado da barra lateral para abrir **Controles do jogo**. Escolha teclado e mouse ou controle para mostrar as dicas daquele dispositivo. Os dois continuam funcionando. Nesse menu você também pode ajustar a opacidade das dicas e trocar de esquema. Em **Sensibilidade do mouse**, X ajusta o movimento horizontal e Y ajusta o vertical ao mirar ou usar o olhar livre com o mouse. O esquema ativo do jogo guarda os dois valores. Use 1,00 para voltar à velocidade original de um eixo.

![Menu de controles do jogo sobre o SuperTuxKart](docs/screenshots/pt-br/controls.png)

**Use vários mapas de controles no mesmo jogo.** Clique em **Gerenciar perfis** no menu do teclado. Escolha **Novo perfil** para começar do zero ou **Duplicar** para copiar as posições e teclas atuais. Dê um nome, como *Veículo*, e clique em **Aplicar**. Em **Editar controles**, arraste os controles para os botões do jogo e clique em **Salvar**. Para trocar durante o jogo, selecione o mapa em **Perfil do jogo** no menu do teclado. Cada jogo lembra o último perfil usado, inclusive depois de fechar o emulador. Em **Gerenciar perfis** também é possível renomear, excluir, importar ou exportar um perfil.

![Gerenciador com dois perfis de controle para o SuperTuxKart](docs/screenshots/pt-br/profiles.png)

Durante a partida, pressione **Ctrl+Shift+P** para ativar o próximo perfil salvo do jogo. Depois do último, o atalho volta ao primeiro. O nome aparece na tela para confirmar a troca.

Selecione **Editor de controles** no menu ou pressione **Ctrl+Shift+A**. Arraste um controle da paleta para a tela do jogo e selecione-o para associar uma entrada e ajustar suas propriedades. Para movimento, solte um **Direcional** sobre o joystick virtual do jogo. Em **Configuração do controle**, associe **Cima**, **Baixo**, **Esquerda** e **Direita** individualmente. Clique no centro do direcional para selecioná-lo e arraste a alça no canto inferior direito para ajustar o raio ao tamanho do joystick. A alça e o painel ajustam o mesmo raio. Em **Analógico**, escolha **Esquerdo** ou **Direito** para usá-lo junto com as teclas de direção. Escolha **Desativado** para usar apenas as direções atribuídas. Clique em **Salvar** para aplicar o esquema.

Para mapas que se movem ao arrastar, solte **Rolagem de borda** em uma área livre do mapa e salve. Mova o ponteiro até uma borda da tela do jogo para mover a câmera naquela direção. Em **Configuração do controle**, ajuste as margens e velocidades de cada eixo. Afaste o ponteiro da borda para parar.

Em jogos que respondem a sensores de movimento, adicione **Inclinar**, associe uma tecla ou botão e escolha o eixo X, Y ou Z e um ângulo de −45° a +45° em **Configuração do controle**. Mantenha a tecla ou o botão pressionado para inclinar o dispositivo virtual a partir da orientação atual. Solte para voltar à orientação original. O controle requer suporte a sensores virtuais.

Em jogos com movimento por clique, solte **Analógico MOBA** sobre o direcional virtual e use **Posicionar herói** para marcar a posição do personagem na tela. Clique com o botão direito no destino para mover o direcional naquela direção. Ajuste o raio e a velocidade do herói para o jogo. Atribua **Parar movimento** se quiser interromper um deslocamento. Use **Habilidade MOBA** para mirar uma habilidade a partir do botão dela no jogo.

Coloque **Olhar livre** sobre o controle de câmera do jogo. Atribua cada direção ou habilite **Olhar com o mouse** e segure a tecla atribuída enquanto move o mouse. Para um aplicativo ou mapa que rola ao arrastar, solte **Rolagem** sobre a área e use a roda do mouse sobre a tela do jogo. Ajuste a velocidade e a amplitude e escolha se a rolagem substitui outros mapeamentos da roda.

Para dirigir com um volante na tela, posicione **Girar** no centro do volante e marque **Volante de um toque**. Ajuste o ângulo inicial do toque, o raio e o ângulo de giro. Com ângulo de giro positivo, a tecla principal gira no sentido horário e **Giro inverso** gira no sentido anti-horário. Cada direção mantém um toque até você soltar a tecla. Por padrão, **Girar** usa dois dedos.

A paleta tem 15 opções para 14 tipos de controle. **Roda do mouse** é um **Deslizar** já configurado para uma direção da roda.

![Editor de controles com um direcional W, A, S, D sobre o menu principal do SuperTuxKart](docs/screenshots/pt-br/editor.png)

*O idioma do SuperTuxKart pode ser alterado nas configurações do próprio jogo.*

## Compilar o projeto

Instale o Rust estável atual com Cargo. No Windows, instale também as ferramentas de compilação de desktop C++ do Visual Studio e o Windows SDK. No Ubuntu 24.04, prepare as dependências nativas:

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libudev-dev libwayland-dev libxkbcommon-dev libx11-dev libxi-dev libxrandr-dev libxcursor-dev libgl1-mesa-dev
```

```sh
git clone https://github.com/felipemarques/RustEmuDroid.git
cd RustEmuDroid
cargo build --locked --release --features ui
cargo run --locked --release --features ui -- ui
```

No Windows, [Abrir RustEmuDroid.cmd](Abrir%20RustEmuDroid.cmd) compila a versão de produção e abre a janela, ou traz a instância existente para frente. Para compilar apenas a ferramenta de linha de comando, omita `--features ui`.

## Linha de comando, dados e logs

```sh
cargo run --locked -- doctor
cargo run --locked -- init --image "system-images;android-35;google_apis;x86_64"
cargo run --locked -- boot --headless --check --gpu auto
cargo run --locked -- boot --help
```

Para usar a imagem do Kemu com dados separados no Windows:

```powershell
cargo run --locked -- --state-dir .rustemudroid/kemu init --system-dir "C:/Program Files/Kemu/system-img"
cargo run --locked --release --features ui -- --state-dir .rustemudroid/kemu ui
```

Use o mesmo `--state-dir` em `init`, `boot` e `ui`. O padrão é `.rustemudroid/windows/` ou `.rustemudroid/linux/` a partir do diretório de trabalho. O `launcher.json` guarda preferências. O perfil ativo contém o AVD e os arquivos `boot-*.log`. O comando `boot --check` aguarda `sys.boot_completed=1` e desliga a VM testada. `--timeout` ajusta o limite de boot em segundos. `--engine sdk` seleciona explicitamente o motor do SDK. O RustEmuDroid escolhe portas livres e verifica o caminho do AVD antes de pedir o desligamento.

## Desempenho e solução de problemas

Os testes no Windows comparam as transições de menu e o campo de treino do PUBG, além do replay do SuperTuxKart, nas GPUs dedicada e integrada. Cada resultado identifica a cena, o contador de quadros e as opções gráficas. [Tabela comparativa de FPS](docs/game-testing.md#fps-comparisons-on-the-windows-test-pc)

Para comparar o PUBG no seu PC, meça a mesma cena no RustEmuDroid e no BlueStacks. O coletor informa quantos quadros novos o Android apresentou por segundo e quanto durou a maior pausa entre eles. [Comandos e método de medição, em inglês](docs/game-testing.md#record-a-game-scene-on-your-own-pc)

O [benchmark da Radeon 780M, em inglês](benchmarks/stk-1.5/radeon-780m/README.md), compara a GPU integrada no RustEmuDroid, Kemu e BlueStacks. O relatório inclui os dados de cada corrida e a confirmação da GPU física.

Para comparar as placas no Windows, abra **FPS WW2 URP → Start Game → Level 1**. Ative **Settings → Display FPS** para ver o contador do jogo. Para testar outra cena com Vulkan, abra **andar → Continue → Begin → Skip** e fique de frente para o portão na chuva. Medimos as duas cenas na Radeon 780M e na RTX 5070 Ti. [Resultados do FPS WW2 URP](docs/game-testing.md#radeon-780m-vulkan-compatibility) · [Resultados e download de ANDAR](docs/game-testing.md#andar) · [Todos os testes, em inglês](docs/game-testing.md)

- **FPS.** Em **Configurações → Contador de FPS**, escolha **Painel Rust** (padrão) para contar os quadros processados pelo painel ou **Superfície do jogo no Android** para contar os quadros que o Android apresentou para o jogo em primeiro plano. Clique em **Salvar** para aplicar na hora. A segunda opção também funciona na prévia do BlueStacks com o ADB conectado. Um traço indica que a medição está começando ou que o Android não identificou uma superfície de jogo. Uma cena parada pode marcar zero. O contador interno de cada jogo mede sua própria taxa de renderização.
- **Exemplo medido.** No replay Black Forest do SuperTuxKart 1.5, o RustEmuDroid atingiu média de 59,93 FPS na qualidade original e 28,90 FPS na alta com Windows, RTX 5070 Ti, Android 16 e OpenGL ES. O relatório traz os parâmetros, os tempos por quadro e os resultados do Kemu e BlueStacks. [Método e medições](benchmarks/stk-1.5/README.md)
- **Tela preta ou vídeo lento.** Execute `doctor`, confira a placa Vulkan e teste GPU do computador ou Automática. Software/SwiftShader ajuda a isolar problemas de driver. No Linux, o motor do SDK usa um caminho de compatibilidade GLES para a imagem do Kemu.
- **Testar a GPU integrada no Windows com duas placas de vídeo.** Escolha a integrada em **Placa de vídeo Vulkan**. O compositor de tela do Android ainda pode usar a outra GPU para OpenGL. Para usar a integrada nos dois, localize o `qemu-system-x86_64.exe` em execução no Gerenciador de Tarefas, adicione esse executável em **Configurações → Sistema → Tela → Elementos gráficos** e selecione a opção com o nome da integrada. Reinicie o Android. A preferência do Windows também vale para outras instâncias que usam o mesmo executável. [Como verificamos as duas seleções](docs/game-testing.md#inventory-on-the-radeon-780m)
- **Falhas na alocação de memória Vulkan na Radeon 780M.** No Windows, selecione o perfil **Android 17 Google Play**, escolha **GPU do computador** ou **Automática** e selecione explicitamente **AMD Radeon 780M Graphics**. Ative **Memória Vulkan compartilhada (Radeon 780M)** em **Gráficos**, salve e reinicie o Android. Esta opção experimental vem desativada, vale por perfil e foi verificada com o Android Emulator SDK 37.3.10.
- **Vários dispositivos ADB.** Liste-os com `adb devices -l` e direcione o diagnóstico com `adb -s <serial>`. O serial ativo aparece na saída do launcher.
- **PUBG MOBILE fecha ao iniciar no Android 17 Google Play.** A partir da v0.2.1, o launcher seleciona o driver ANGLE integrado para `com.tencent.ig` nessa imagem x86-64 com suporte a Vulkan e renderização automática ou pela GPU do computador. Uma preferência existente para o jogo tem prioridade. No Linux, a v0.13.3 deixa de adicionar a preferência quando o log da sessão confirma que o Vulkan selecionou um dispositivo de CPU. Em uma RTX 5070 Ti, isso resolveu a falha no driver EGL ao apresentar um quadro. [Seleção de driver por aplicativo no Android](https://developer.android.com/games/develop/vulkan/overview)
- **PUBG fica nos logos de abertura no Android 16 do Kemu.** Na engrenagem, abra **Compatibilidade de jogos**, ative **Tradução ARM leve para PUBG** e salve. Se o Android estiver aberto, use **Salvar e reiniciar Android**. No computador de teste, o jogo chegou ao aviso de atualização em 56 segundos e abriu a tela de boas-vindas depois do download. O ajuste vale para todos os aplicativos ARM durante essa inicialização. [Teste de abertura no Android 16](docs/game-testing.md#pubg-startup-on-kemu-android-16)
- **Jogos ausentes depois de trocar o perfil.** Volte ao perfil onde os instalou. Instalações novas começam sem seus jogos ou contas.

## Arquitetura e testes

| Componente | Responsabilidade |
|---|---|
| [Interface desktop](src/ui.rs) | Rust, eframe/egui e Glow. Gerencia a janela, os controles, as configurações e a tela de encerramento. |
| [Ciclo de vida da VM](src/android.rs) | Localiza o SDK e as imagens, cria o AVD, inicia o motor escolhido e aguarda o fim do processo. |
| [Vídeo e entrada](src/display.rs) | Recebe quadros pelo gRPC local autenticado de `EmulatorController.streamScreenshot`, guarda o quadro RGBA pendente mais recente e encaminha eventos de toque e teclado. |
| [Home do Android](android/home) | Activity nativa que consulta o PackageManager e exibe banners ou ícones dos apps. O cliente instala o APK de desenvolvimento embutido via ADB. |

Execute `cargo test --locked --features ui` para rodar a suíte offline. O [workflow do GitHub Actions](.github/workflows/build.yml) testa e compila executáveis release para Windows e Ubuntu x86-64 em pushes, pull requests e execuções manuais. Os testes com Android real precisam de um AVD local. Para repetir o teste de fechamento da janela no Windows, aponte `RUSTEMUDROID_TEST_STATE` para o diretório absoluto de um perfil ocioso e execute `cargo test --locked --features ui real_window_paints_until_android_exits -- --ignored --nocapture`.

Para recompilar a Home, instale um JDK, o `android.jar` de uma plataforma e o Android Build-Tools. Use `python android/home/build.py <android.jar> <diretório-do-build-tools>`. O script usa `~/.android/debug.keystore` por padrão e aceita `--keystore` e `--alias`. Para atualizar uma instalação existente, mantenha a chave de assinatura e incremente o código da versão no manifesto e na [verificação de instalação](src/controls.rs).
