# RustEmuDroid

Protótipo de launcher em Rust para iniciar uma VM Android isolada. Usa o executável Android Emulator do SDK e confirma a inicialização pelo ADB com `sys.boot_completed=1`.

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

A imagem testada é o Android 16 (API 36) do Kemu, build BP4A.251205.006, com a ABI de sistema x86-64. Para essa imagem, o launcher seleciona o driver OpenGL ES que já vem no Android e ajusta persist.graphics.egl por ADB no disco do protótipo. Essa configuração permite inicializar o compositor com o motor do SDK. O perfil de uma imagem instalada via SDK usa a configuração gráfica padrão do Android Emulator.

## Usar uma imagem do SDK

Para usar o perfil padrão do SDK, instale os pacotes pelo SDK Manager ou execute o `sdkmanager` com as ferramentas de linha de comando do Android no `PATH`.

```text
sdkmanager --install "emulator" "platform-tools" "system-images;android-35;google_apis;x86_64"
cargo run -- init
cargo run -- boot --headless --check
```

`init --image` aceita outro pacote x86-64 instalado das variantes `default` ou `google_apis`. Use `--state-dir` com um diretório diferente para manter perfis com imagens diferentes.

## Interface

O painel do RustEmuDroid reúne o caminho do SDK, a imagem Android, o perfil e o modo de renderização. O botão **Abrir Android** inicia a janela do Android. **Verificar boot** faz um teste sem janela e encerra a VM ao confirmar a inicialização.

```text
cargo run --locked --features ui --target-dir target/painel -- ui
```

No Windows, o painel preenche o caminho de instalação padrão do Kemu quando encontra a imagem. No Linux, informe a pasta da imagem ou deixe o campo em branco para usar o perfil existente ou a imagem padrão do SDK. O painel usa o mesmo launcher da linha de comando. A janela abriu no Windows, o executável do painel confirmou o boot e o código da interface passou na verificação de compilação no Ubuntu em WSL2.

## Rodar e diagnosticar

- `doctor` localiza o SDK com `--sdk`, `ANDROID_HOME`, `ANDROID_SDK_ROOT` ou o caminho padrão `Android/Sdk` da plataforma. Ele verifica se o hipervisor pode ser usado.
- `init` prepara um AVD com 4 vCPUs, 3 GiB de RAM e resolução de 1080 × 1920.
- `boot` imprime o serial, o log e o tempo de inicialização. Sem `--headless`, abre a janela do Android Emulator e acompanha a VM enquanto ela estiver aberta.
- `boot --headless --check` testa a inicialização e encerra a VM do protótipo após o Android confirmar o boot. `--timeout` ajusta a espera em segundos.

Os dados graváveis e os logs ficam em `.rustemudroid/windows` ou `.rustemudroid/linux`. O launcher escolhe um par livre de portas de console e ADB e confere o nome do AVD antes de consultar o Android ou pedir o desligamento. Uma imagem do Kemu é usada como base para um disco novo no perfil do protótipo.

## Validação local

| Ambiente | Aceleração | Primeiro boot |
|---|---|---:|
| Windows 11 x86-64 | WHPX e OpenGL ES no host, com RTX 5070 Ti | 71,7 s |
| Ubuntu 24.04 no WSL2 x86-64 | KVM e SwiftShader | 90,8 s |

Os dois testes usaram perfis novos, executaram `boot --headless --check` e retornaram `sys.boot_completed=1` com a mesma fingerprint da imagem do Kemu. Na verificação Windows, Kemu e BlueStacks continuaram conectados nas portas anteriores. Os três testes do pacote Rust também passaram nas duas plataformas.

A validação cobre a inicialização do Android. A interface própria em Rust, a compatibilidade de jogos e o desempenho de Vulkan permanecem fora deste protótipo.


[Comparação em jogo com SuperTuxKart 1.5](benchmarks/stk-1.5/README.md)

## Referências

- [Comandos do Android Emulator](https://developer.android.com/studio/run/emulator-commandline)
- [Aceleração no Windows e Linux](https://developer.android.com/studio/run/emulator-acceleration)
- [Diretórios e variáveis do Android SDK](https://developer.android.com/tools/variables)
