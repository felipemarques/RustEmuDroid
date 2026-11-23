# SuperTuxKart 1.5 no Windows

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
