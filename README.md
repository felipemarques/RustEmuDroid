# RustEmuDroid

Interface em Rust para jogar Android em uma janela no Windows e no Linux. O painel inicia a VM, exibe a tela do Android e encaminha mouse e teclado. No Windows, a instalação do Kemu pode fornecer a imagem Android 16 e o motor QEMU/gfxstream com aceleração Vulkan. O Android Emulator do SDK também é suportado.

## Abrir para jogar

No Windows, dê dois cliques em [Abrir RustEmuDroid.cmd](Abrir%20RustEmuDroid.cmd). O atalho compila a versão de lançamento e abre a sessão integrada. Também é possível abrir o painel pela linha de comando nas duas plataformas.

```text
cargo run --locked --release --features ui -- ui --play
```

Clique e arraste sobre o Android para tocar e deslizar. O painel tem controles para voltar, ir à tela inicial e abrir apps recentes. O contador mostra quantos quadros novos a interface recebeu e enviou para desenho por segundo. O botão **Encerrar Android** encerra a VM e volta ao painel.

## Pré-requisitos

- Windows ou Linux x86-64, com WHPX ou KVM utilizável, respectivamente.
- Rust com a edição 2024 e Android SDK com Android Emulator e Platform-Tools.
- Imagem Android x86-64. Use a pasta `system-img` do Kemu ou instale uma imagem pelo SDK Manager.

## Usar a imagem do Kemu

Execute na raiz do repositório. Estes comandos usam o caminho padrão do Kemu no Windows.

```powershell
cargo run -- doctor
cargo run -- init --system-dir 'C:/Program Files/Kemu/system-img'
cargo run -- boot --headless --check --gpu host
```

No Linux, informe onde está a pasta com a mesma imagem. O exemplo usa a partição Windows montada pelo WSL2.

```bash
cargo run -- doctor
cargo run -- init --system-dir '/mnt/c/Program Files/Kemu/system-img'
cargo run -- boot --headless --check --gpu swiftshader
```

No WSL, use o SDK Linux e uma conta com acesso de leitura e escrita a /dev/kvm. Se o SDK estiver fora do diretório padrão, informe --sdk ou ANDROID_HOME.

A imagem testada é o Android 16 (API 36), build BP4A.251205.006, x86-64. No Windows, com GPU automática ou do host, o launcher encontra o motor ao lado da pasta `system-img`. A interface em Rust inicia diretamente o QEMU dessa instalação. O Android usa ANGLE e Vulkan com as filas de memória compartilhada implementadas pelo motor. A VM de jogos usa 4 vCPUs, 8 GiB e 1600 × 900 a 60 Hz. O disco gravável fica em `<perfil>/gaming` e precisa de sua própria instalação dos jogos.

Para uma instalação do Kemu fora de `Program Files/Kemu`, defina `RUSTEMUDROID_KEMU_ROOT` com o diretório da instalação e selecione sua pasta `system-img`. O perfil anterior do SDK continua acessível com `boot --engine sdk`. No Linux e em instalações que usam somente o SDK, a imagem do Kemu usa o driver GLES de compatibilidade. Nesse caso, o launcher configura `persist.graphics.egl` no disco do protótipo. As imagens padrão do SDK usam o driver escolhido pelo próprio Android Emulator.

## Usar uma imagem do SDK

Para usar o perfil padrão do SDK, instale os pacotes pelo SDK Manager ou execute o `sdkmanager` com as ferramentas de linha de comando do Android no `PATH`.

```text
sdkmanager --install "emulator" "platform-tools" "system-images;android-35;google_apis;x86_64"
cargo run -- init
cargo run -- boot --headless --check
```

`init --image` aceita outro pacote x86-64 instalado das variantes `default` ou `google_apis`. Use `--state-dir` com um diretório diferente para manter perfis com imagens diferentes.

## Interface

Execute o comando abaixo para editar os caminhos do SDK, da imagem e do perfil antes de jogar. **Abrir Android** exibe o Android no mesmo painel. **Verificar boot** inicia a VM em segundo plano e encerra o processo assim que `sys.boot_completed=1`.

```text
cargo run --locked --release --features ui -- ui
```

O painel usa egui/eframe e recebe quadros RGBA do serviço `EmulatorController.streamScreenshot` em loopback. Cada sessão usa uma porta livre e o token do seu próprio AVD. Há espaço para um quadro pendente. O painel usa o mais recente a cada atualização. Mouse e teclado seguem pelo mesmo serviço. Para validar o painel no WSL2 com renderização por software, use `ui --play --gpu swiftshader`.

## Rodar e diagnosticar

- `doctor` localiza o SDK com `--sdk`, `ANDROID_HOME`, `ANDROID_SDK_ROOT` ou o caminho padrão `Android/Sdk` da plataforma. Ele verifica se o hipervisor pode ser usado.
- `init` registra a imagem e prepara o perfil do SDK com 4 vCPUs, 3 GiB e 1080 × 1920.
- `boot` imprime o serial, o log e o tempo de inicialização. Sem `--headless`, abre a janela do Android Emulator e acompanha a VM enquanto ela estiver aberta.
- `boot --headless --check` testa a inicialização e encerra a VM do protótipo após o Android confirmar o boot. `--timeout` ajusta a espera em segundos.

Os dados graváveis e os logs ficam em `.rustemudroid/windows` ou `.rustemudroid/linux`. O launcher escolhe um par livre de portas e verifica o caminho do AVD antes de configurar o Android ou pedir o desligamento.

## Validação local

| Ambiente | Aceleração | Primeiro boot |
|---|---|---:|
| Windows 11 x86-64 | WHPX e OpenGL ES no host, com RTX 5070 Ti | 71,7 s |
| Ubuntu 24.04 no WSL2 x86-64 | KVM e SwiftShader | 90,8 s |

Os dois testes usaram perfis novos, executaram `boot --headless --check` e retornaram `sys.boot_completed=1` com a mesma fingerprint da imagem do Kemu. Na verificação Windows, Kemu e BlueStacks continuaram conectados nas portas anteriores. Os três testes do pacote Rust também passaram nas duas plataformas.

O painel integrado exibiu quadros do Android no Windows e no Ubuntu 24.04 em WSL2. No Windows, o toque abriu as configurações de rede e o botão Início devolveu o Android ao launcher. O teste de desempenho usa SuperTuxKart no Windows com a mesma imagem e a mesma placa de vídeo do Kemu.


[Comparação em jogo com SuperTuxKart 1.5](benchmarks/stk-1.5/README.md)

## Referências

- [Comandos do Android Emulator](https://developer.android.com/studio/run/emulator-commandline)
- [Aceleração no Windows e Linux](https://developer.android.com/studio/run/emulator-acceleration)
- [Diretórios e variáveis do Android SDK](https://developer.android.com/tools/variables)
