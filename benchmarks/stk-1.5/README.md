# SuperTuxKart 1.5 no Windows

[Comparativo de desempenho do primeiro protótipo até a v0.21.1](../../docs/performance-history.pt-BR.md) · [Performance history in English](../../docs/performance-history.md)

[Radeon 780M integrated graphics benchmark (English)](radeon-780m/README.md)

[Optional SuperTuxKart Vulkan frame profiling (English)](vulkan-end-scene.md)

## Home do Android e imagem Google Play

Em 23/11/2026, o painel integrado executou o replay Black Forest na RTX 5070 Ti a 1600 × 900, escala de 100%, 4 vCPUs, 8 GiB e limite de 60 FPS. A tabela mostra a média de duas corridas por célula. O P95 usa os tempos positivos da coluna `Main loop` das duas corridas no perfil alto.

| Configuração | Original (FPS) | Alto (FPS) | Alto, P95 (ms) |
|---|---:|---:|---:|
| Android 16 do Kemu, biblioteca antiga, OpenGL ES | 59,59 | 28,18 | 47,31 |
| Android 16 do Kemu, nova Home, OpenGL ES | 59,94 | 28,42 | 47,52 |
| Android 17 com Google Play, nova Home, OpenGL ES padrão | 31,03 | 14,00 | 95,39 |
| Android 17 com Google Play, nova Home, OpenGL ES por ANGLE | 55,06 | 25,36 | 54,19 |
| Android 17 com Google Play, nova Home, Vulkan direto | 55,91 | 56,15 | 24,01 |

O par Android 16 manteve a imagem, o motor e a API gráfica para medir a mudança de Home. O Android 17 usou a imagem Google Play API 37.0 e o Android Emulator 37.3.10. A mudança de imagem também mudou o motor e o driver. O jogo registrou os mesmos 21 parâmetros nas corridas OpenGL. No alto com Vulkan, registrou filtragem anisotrópica 16×, frente aos 8× das corridas OpenGL. Os resultados medem quadros do jogo. O perfil Google local passou a usar Vulkan direto no SuperTuxKart ao fim do ensaio.

Para repetir em um único AVD, use `python benchmarks/stk-1.5/compare.py --rust emulator-5554 --rounds 2 --presets original high --output resultados-gl`. Para Vulkan, use `--presets vulkan high-vulkan` e escolha outro diretório de saída. [Medições individuais](home-android/medicoes.csv) · [Relatórios de quadros e drivers](home-android/relatorios.zip)

## RustEmuDroid, Kemu e BlueStacks no replay Black Forest

Em 23/11/2026, os três emuladores executaram o mesmo APK e replay a 1600 × 900, escala de 100%, 4 vCPUs, 8 GiB e limite de 60 FPS na RTX 5070 Ti. O jogo usou OpenGL ES em todos. Cada célula de FPS é a média de três corridas. A ordem dos emuladores mudou a cada rodada e apenas um executou o jogo por vez. A contagem vem do próprio jogo.

| Emulador | Perfil original | Qualidade alta | Tempo por quadro no alto, P95 |
|---|---:|---:|---:|
| RustEmuDroid | 59,93 FPS | 28,90 FPS | 45,37 ms |
| Kemu 0.7.1 | 59,98 FPS | 28,68 FPS | 46,56 ms |
| BlueStacks 5.22.166.1003 Pie64 | 60,02 FPS | 59,99 FPS | 17,70 ms |

O P95 usa os tempos positivos da coluna `Main loop` do profiler nas três corridas. No perfil alto, 95% dos quadros do RustEmuDroid levaram até 45,37 ms. Os relatórios das três plataformas registraram os mesmos 21 parâmetros gráficos por perfil. O perfil alto solicitou sombras de 1024, SSAO, bloom, feixes de luz e filtragem anisotrópica 8×. Os tempos de execução registram a passagem de sombras nos três. As imagens finais podem variar com o driver.

RustEmuDroid e Kemu rodaram Android 16 e código x86-64, com OpenGL ES 3.1 via ANGLE. O BlueStacks rodou Android 9 e a variante ARM de 32 bits do mesmo APK, com OpenGL ES 3.2. A variante ARM64 encerrou na abertura e a instalação forçada de x86-64 no Pie64 deixou a SDL ausente. A reinstalação do mesmo APK preservou os dados. O SHA-256 verificado nos emuladores foi `e1f2f49152abfb8f3ed0ced47981e84096758bff9758563aa3b917111a2a3cbd`.

No perfil alto, a espera mediana em `Sync Stall` foi de 20,21 ms no RustEmuDroid, 22,44 ms no Kemu e 0,18 ms no BlueStacks. O jogo espera nessa região pela conclusão de trabalho gráfico anterior. A medição abrange toda a instalação, incluindo as diferenças de Android, arquitetura e driver.

- [Resultados das 18 corridas](tres-emuladores/medicoes.csv)
- [Relatórios, tempos individuais de quadro e identificação dos drivers](tres-emuladores/relatorios.zip)

O script aceita `--bluestacks` com o serial ADB local do App Player, além de `--kemu` e `--rust`. Consulte `adb devices -l` para identificar as três instâncias e use `--presets original high --rounds 3`.

## Painel integrado com aceleração Vulkan

O RustEmuDroid passou de **45,86 para 59,98 FPS** no perfil gráfico original. A nova série usou o painel visível, Android 16 x86-64, 4 vCPUs, 8 GiB, RTX 5070 Ti, 1600 × 900, escala de 100% e limite de 60 FPS nos dois emuladores. O Kemu v0.7.1 e o RustEmuDroid usaram a mesma instalação do QEMU/gfxstream, com discos Android independentes. O runner alternou a ordem entre as rodadas. Cada célula é a média de três execuções do replay Black Forest.

| Renderização do SuperTuxKart | Kemu | RustEmuDroid | Tempo por quadro no RustEmuDroid |
|---|---:|---:|---:|
| OpenGL ES via ANGLE, perfil original | 59,90 FPS | 59,98 FPS | 16,67 ms |
| OpenGL ES via ANGLE, qualidade alta | 29,24 FPS | 29,27 FPS | 34,17 ms |
| Vulkan direto, perfil original | 60,01 FPS | 60,01 FPS | 16,66 ms |

O perfil alto usou sombras de 1024, SSAO, bloom, feixes de luz, filtragem anisotrópica 8× e mais geometria. Ele ficou abaixo do teto de 60 FPS nos dois emuladores. As diferenças entre as médias ficaram dentro da variação observada entre as corridas. O resultado se aplica a esse replay, hardware e versões.

O jogo calcula FPS a partir dos quadros e da duração do replay. No RustEmuDroid também contei os quadros enviados à textura do painel durante aproximadamente 36 segundos de cada corrida OpenGL ES. As três médias ficaram entre **59,72 e 60,08** no perfil original e entre **28,51 e 28,74** no alto. Essa segunda contagem mede a etapa de exibição do painel. O ensaio de Vulkan direto confirmou o driver escolhido no log do próprio jogo.

- [Medições das 18 corridas](integrado/medicoes.csv)
- [Relatórios originais do jogo e identificação do driver Vulkan](integrado/relatorios.zip)
- [Script de comparação](compare.py)

Para reproduzir, abra cada emulador, instale o mesmo APK x86-64 do SuperTuxKart 1.5 e abra o jogo uma vez para gerar sua configuração. Confira os seriais com `adb devices`. O script verifica os nomes dos AVDs, restaura o arquivo de configuração depois de cada corrida e exige um diretório de saída novo. O SHA-256 do APK usado foi `e1f2f49152abfb8f3ed0ced47981e84096758bff9758563aa3b917111a2a3cbd`.

```text
python benchmarks/stk-1.5/compare.py --kemu emulator-5556 --rust emulator-5558 --presets original high vulkan --rounds 3 --output resultados
```

## Medição anterior com o motor do SDK

Replay `benchmark_black_forest.replay`, com o mesmo APK, ABI x86-64, resolução de 1600 × 900, escala interna de 100% e limite de 60 FPS. O perfil gráfico do Kemu foi aplicado temporariamente aos dois testes e restaurado ao concluir. Foi feita uma corrida por emulador, em sequência, na RTX 5070 Ti.

| Emulador | Quadros | Duração | FPS médio | Tempo médio por quadro |
|---|---:|---:|---:|---:|
| Kemu | 2.282 | 38,228 s | 59,69 | 16,75 ms |
| RustEmuDroid | 1.747 | 38,098 s | 45,86 | 21,81 ms |
| BlueStacks Pie64 | — | — | — | — |

O RustEmuDroid registrou cerca de 23% menos FPS nesta corrida. O jogo usou OpenGL ES nos dois testes. O Kemu encaminhou a renderização via ANGLE e o protótipo usou o tradutor OpenGL ES do Android Emulator. Os perfis de máquina virtual mantiveram as configurações de RAM existentes. O FPS foi calculado dividindo os quadros pela duração de cada replay.

O processo do BlueStacks encerrou a VM na primeira tentativa. Após reiniciar, o Android chegou à tela inicial, mas as tentativas por ADB retornaram `error: closed` e o atalho do jogo não abriu a corrida. A instância não produziu relatório.

## Relatórios

- [Kemu](Kemu-perf-report-black_forest.csv)
- [RustEmuDroid](RustEmuDroid-perf-report-black_forest.csv)
- [Opções de benchmark do SuperTuxKart](https://github.com/supertuxkart/stk-code/blob/master/src/main.cpp)
