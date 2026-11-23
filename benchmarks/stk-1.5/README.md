# SuperTuxKart 1.5 no Windows

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
