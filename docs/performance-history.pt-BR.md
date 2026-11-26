# Desempenho do RustEmuDroid, Kemu e BlueStacks no Windows

**Relatório de 25 de novembro de 2026.** O primeiro protótipo entregou **45,86 FPS** no replay Black Forest do SuperTuxKart. Ainda na v0.1.0, o RustEmuDroid passou a **59,98 FPS** no perfil Original, próximo dos **59,90** do Kemu. Mais tarde, com o jogo em qualidade Alta e a tela Android a 90 Hz, a interface Rust chegou a **90,02 FPS** usando o motor e a imagem Android 16 do Kemu. Com o perfil próprio Android 17 Google Play, a v0.21.1 marcou **85,62 FPS**, ao lado de **90,02 no Kemu** e **89,99 no BlueStacks**. [PDF em português](../output/pdf/comparativo-de-desempenho-2026-11-25.pdf) · [English](performance-history.md)

**26 de novembro de 2026.** Em Vulkan Mailbox sem limite na RTX 5070 Ti, um motor Android Emulator 37.3.10 com patches, junto com ajustes no host e no convidado, levou o SuperTuxKart na imagem oficial Android 17 Google Play, sem modificações, de **164–176 FPS** para **356 FPS** no modo de energia Equilibrado do Windows e **375 FPS** no modo Melhor desempenho. Na mesma sessão, o Kemu marcou **325 e 358** e o BlueStacks **376 e 396**. Um operador de testes configurou o ambiente do motor em volta do cliente v0.22.0. Numa verificação posterior, durante o dia e com as mesmas fontes do motor, o cliente v0.23.0 aplicando os ajustes a partir das próprias preferências mediu o mesmo que o caminho do operador (341,8 contra 342,5 FPS). [Caminho do cliente e do operador](#caminho-do-cliente-e-do-operador) [Condições, receita e limites](#motor-de-desempenho-em-mailbox-sem-limite) · [Kit de fontes do motor, em inglês](../experiments/whpx-clock/README.md#4-build-the-performance-engine)

Na mesma **janela Rust da v0.21.1**, uma compilação experimental do motor Android com o relógio virtual corrigido elevou o jogo de **85,65 para 87,46 FPS (+2,11%)** em quatro corridas. A proporção dos intervalos de apresentação do Android acima de 16,667 ms caiu de **5,08% para 3,18%**. A versão publicada ainda usa a configuração habitual do motor. [Resultados por corrida e tempos de quadro](../benchmarks/stk-1.5/windows-whpx-clock-kart.md)

Com o cliente v0.22.0 e a nova compilação do motor Windows, o mesmo replay do kart marcou **84,73 FPS com o cache desligado e 90,16 com ele ligado**. O P99 do Android caiu de 23,17 para 14,46 ms. O painel Rust processou 86,32 quadros/s com o cache ligado. O par de corridas verificou a compilação feita com as fontes do FFmpeg e do libvpx. [Identidade do motor, método e dados](../benchmarks/stk-1.5/windows-source-media-cache-rtx.md)

Em duas corridas Mailbox por motor, o jogo marcou **163,54–176,38 FPS no RustEmuDroid**, **289,67–294,92 no Kemu** e **350,42–372,41 no BlueStacks**. A SurfaceView do Android ficou perto de 90 apresentações/s nos três. A taxa de quadros gerados pelo jogo e a cadência do Android vêm de contadores separados. [Seis corridas Mailbox comparáveis](../benchmarks/stk-1.5/windows-rtx-mailbox-peers.md)

Com o **cliente oficial v0.22.0 para Windows e o Android Emulator 37.3.10 de fábrica**, ligar `VulkanAllocateHostMemory` reduziu o desempenho do SuperTuxKart de **85,74 para 71,76 FPS (−16,31%)**. O P99 do Android passou de **22,91–23,41 ms** para **41,91–42,05 ms**. As quatro inicializações usaram o mesmo jogo, a mesma RTX e a mesma imagem. Uma série anterior na v0.21.1 também registrou queda com essa opção. A sonda sintética daquela série tinha medido escritas de 32 bits mais rápidas, de 91,843 para 0,290 ms por 4 MiB. O cache do motor recompilado usa outro caminho de alocação. [Cliente atual e tempos de quadro](../benchmarks/stk-1.5/windows-stock-vulkan-host-memory-replay-v0220-rtx.md) · [Série histórica na v0.21.1](../benchmarks/stk-1.5/windows-stock-vulkan-host-memory-replay-rtx.md) · [Sonda sintética](../benchmarks/stk-1.5/windows-stock-vulkan-host-memory.md)

Em outro teste, o motor recompilado decodificou vídeo por software e gravou VP9. Os três decodificadores Goldfish anunciados como acelerados retornaram erro no caminho testado. [Teste de mídia](../benchmarks/stk-1.5/windows-source-media-cache-rtx.md#media-smoke-on-the-relinked-engine)

No Cipó Livre, a 1600 × 900 e 60 Hz na RTX, o RustEmuDroid apresentou **59,68–59,98 quadros/s** e o Kemu **59,85–60,04** com o mesmo APK ajustado localmente. Ao final dos trechos na RTX, o jogo mostrava escala de renderização 3D de 80%. Na Radeon 780M integrada, o Rust apresentou **55,13 e 57,19 quadros/s** nas caminhadas, que terminaram com escala de 80% e 70%. [Ajustes de áudio, cenas e tempos de quadro](../benchmarks/cipo-livre/README.md)

## Motor de desempenho em Mailbox sem limite

Todas as corridas de 25 e 26 de novembro usaram o mesmo PC com Windows (Ryzen 9 8945HS, oito núcleos, GeForce RTX 5070 Ti), o replay automático Black Forest do SuperTuxKart 1.5 em Alta, Vulkan com o VSync do jogo desligado (Mailbox, sem limite), 1600 × 900 no Android, tela Android a 90 Hz e quatro vCPUs. O FPS vem do relatório do profiler do próprio jogo: quadros sobre a duração da volta medida. Cada valor abaixo é a média das voltas. O Android usou a imagem oficial Android 17 Google Play x86-64, revisão 7, sem modificações. A janela Rust era o **cliente desktop v0.22.0**, com painel de 800 × 450. Um operador de testes abriu o cliente e definiu ele mesmo o ambiente do motor, as chaves do AVD e o modo de energia, um boot a frio por vez, com uma volta de aquecimento e duas voltas medidas por boot. A predefinição da **v0.23.0** nas Configurações aplica os mesmos valores; uma verificação separada abaixo compara os dois caminhos. A partir do passo 6, cada boot esperou o QEMU ficar abaixo de 0,6 núcleo do host antes de instalar o jogo.

O motor é o Android Emulator 37.3.10 (`emulator.exe` original) com o núcleo QEMU recompilado (SHA-256 `a55d48c6…37f5`) e o renderizador gfxstream recompilado (`d92f6060…60d0`). O kit publicado descreve uma compilação posterior das mesmas fontes (QEMU `1c7a05e3…a0c6`, gfxstream `7d01cde7…85fe`). Ela grava na hora as linhas de confirmação do motor e acrescenta quatro opções que ficam desligadas sem a sua variável e não fizeram parte destas medições. [Kit de fontes, opções e compilação, em inglês](../experiments/whpx-clock/README.md#4-build-the-performance-engine)

| Modo de energia do Windows | RustEmuDroid, motor de desempenho | Kemu 0.7.1 · Android 16 | BlueStacks 5 Pie64 · Android 9 |
|---|---:|---:|---:|
| Equilibrado | **356** (1 rodada, 2 boots, 4 voltas) | 325 (3 boots, 6 voltas) | 376 (1 boot, 2 voltas) |
| Melhor desempenho | **375** (3 rodadas, 6 boots, 12 voltas) | 358 (1 boot, 2 voltas) | 396 (1 boot, 2 voltas) |

O mesmo operador mediu Kemu e BlueStacks na mesma sessão, entre as rodadas do RustEmuDroid, cada um no seu próprio player e janela. Os valores de Melhor desempenho dos dois e o valor do BlueStacks em Equilibrado vêm de um boot cada. No começo da noite, o mesmo cliente e a mesma carga marcaram 164–176 FPS com o motor original. O RustEmuDroid ficou 9,5% à frente do Kemu em Equilibrado e 4,7% em Melhor desempenho. O BlueStacks continuou 5–6% à frente nos dois modos. A meta de passar o Kemu e os 300 FPS foi atingida; alcançar o BlueStacks, não.

### Receita e efeito medido de cada passo

Cada passo foi medido contra a configuração anterior numa mesma rodada alternada (A/B/B/A ou parecida, mesmo binário). A mesma configuração variou 2–3% entre rodadas, e voltas isoladas variaram 5–8%. Os passos 1–9 usaram o modo Equilibrado, salvo indicação.

| # | Mudança | Como é ativada | Resultado medido |
|---|---|---|---|
| 1 | TSC do convidado sincronizado no WHPX | `RUSTEMUDROID_QA_WHPX_SYNC_TSC=1` (QEMU) | 179,53 → 228,12 FPS com o cache Vulkan ligado. Outro trabalho pesado em disco coincidiu com essa rodada. Rodadas limpas posteriores de cache + relógio marcaram 245,57 e 248,03. |
| 2 | Memória Vulkan visível ao host com cache (tipo 2) | `RUSTEMUDROID_QA_VULKAN_CACHED_HOST=1` (QEMU) | Presente em todas as rodadas; não medido à parte em Mailbox |
| 3 | Transporte gráfico por memória compartilhada com consumidor do host em espera ativa | AVD `hw.gltransport=asg` + `RUSTEMUDROID_QA_ASG_SPIN_US` (gfxstream) | Só ASG: 245,57 → 247,27. ASG + espera de 200 µs: 244,02 → 260,45 |
| 4 | Composição Vulkan com ANGLE no convidado | `-feature VulkanNativeSwapchain` (recurso original) | 253,63 → 269,05. Esse braço rodou por último. |
| 5 | Memória com cache nas alocações Vulkan device-local mapeáveis | `RUSTEMUDROID_QA_VULKAN_CACHED_DEVICE_LOCAL=2` (gfxstream) | 268,42 → 310,25 (três boots cada) |
| 6 | Parâmetros do kernel do convidado | AVD `kernel.parameters=mitigations=off init_on_alloc=0 transparent_hugepage=always` | 301,54 → 310,08 (+2,8%, perto do ruído) |
| 7 | vCPUs em núcleos físicos dedicados, nunca o núcleo 0 | `RUSTEMUDROID_QA_VCPU_PLACEMENT=sets-high` (QEMU) | `sets` (núcleos 0–3): 304,46 → 341,84. `sets-high` (núcleos 4–7) no lugar de `sets`: 314,0 → 345,7 |
| 8 | Menos saídas da VM | `RUSTEMUDROID_QA_LAPIC_FASTPATH=1`, `RUSTEMUDROID_QA_KVM_PV_EOI=1` (QEMU) | 327,51 → 339,83 e 332,57 → 341,31, ambos dentro do ruído. O tempo no tratamento do LAPIC caiu de 86 para 43 ms/s. |
| 9 | Espera ativa mais longa do consumidor | `RUSTEMUDROID_QA_ASG_SPIN_US=3000` em vez de 200, com Melhor desempenho | 373,7 → 384,6 (+2,9%) |
| 10 | Modo de energia Melhor desempenho do Windows enquanto o Android roda | Sobreposição de energia `ded574b5-45a0-4f42-8737-46345c09c238` | Com `sets-high`: 345,7 → 362,7. Receita final: 355,5 → 359,6 incluindo uma volta de 300,90, 379,2 sem ela |

Com `sets`, o resultado dependia da vCPU que executava a thread principal do jogo: 330–375 FPS nas vCPUs 2–3 e 270–305 FPS na vCPU ligada ao núcleo 0 do host. O `sets-high` eliminou essa divisão. Durante as voltas medidas, o QEMU usou cerca de 1,8–2,0 núcleos do host com espera de 200 µs e 2,7 com 3000 µs. A vCPU mais ocupada, que executava a thread principal do jogo, ficou perto de um núcleo inteiro.

**Testado e descartado:** agrupamento de descritores Vulkan (`VulkanBatchedDescriptorSetUpdate`, 379,8 → 355,7); espera no pipe OpenGL ES segurando a trava global mais espera ativa da thread de renderização (227,12 → 172,2, um boot cada); `idle=poll` no convidado (319,74 → 293,66); polling no HLT (339,83 → 291,50); posicionamento `ideal` das vCPUs (320,94 → 305,05); esperas do consumidor de 5000 µs e 1000 µs; buffers ASG maiores (260,45 → 227,48); desligar a RAM do convidado mapeada em arquivo (304,19 → 294,57); liberar a trava na leitura reduzida da tela, a verificação sem trava antes de entrar na VM e prioridade acima do normal para as vCPUs (todos dentro do ruído); a opção original `VulkanAllocateHostMemory` (−16% numa série FIFO anterior); `isolcpus=0` no kernel do convidado (332 FPS contra 337 do controle numa série durante o dia, sem ganho). A RAM do convidado em páginas grandes do host não foi medida: a conta não tem o privilégio "Bloquear páginas na memória", e uma sonda de memória no convidado mediu acesso aleatório mais lento no BlueStacks do que neste convidado, o que tirou a motivação.

**O que ainda falta.** Com Melhor desempenho, o laço principal levou 2,633 ms por quadro contra 2,527 ms no BlueStacks. A atualização gráfica completa ficou quase igual (2,393 contra 2,346 ms). O BlueStacks gastou menos tempo em `drawAll` e na lógica do jogo, e mais em `endScene`.

**Sensibilidade à carga em segundo plano.** Em 26 de novembro, durante o dia, com outros programas da área de trabalho usando cerca de 0,7 a 1,2 núcleo do host, a mesma receita e os mesmos binários marcaram 309 a 350 FPS por volta, 313 e 337 no conjunto de duas séries, em vez de cerca de 375 com Melhor desempenho. As voltas acompanharam a carga: com cerca de 1,0 núcleo ocupado, 309–331 FPS; com cerca de 0,75, 343–350. Os números da noite foram medidos com a área de trabalho ociosa. Kemu e BlueStacks não foram medidos com essa carga, então esses valores do dia não comparam os emuladores.

### Caminho do cliente e do operador

Em 26 de novembro, entre 10h59 e 11h13, o mesmo PC, a mesma RTX 5070 Ti, a mesma imagem e a mesma carga do SuperTuxKart rodaram uma série alternada o, c, c, o, com duas voltas medidas por boot e o modo de energia Melhor desempenho. Os dois braços usaram como janela Rust uma compilação de pré-lançamento do cliente v0.23.0 (as mesmas fontes, compiladas antes de o número da versão subir de 0.22.0) e a compilação do motor com as fontes publicadas (QEMU `1c7a05e3…a0c6`, gfxstream `7d01cde7…85fe`, opções novas desligadas). No braço o, o operador de testes aplicou os ajustes: ambiente, recurso do emulador, chaves do AVD e modo de energia. No braço c, o cliente aplicou os ajustes a partir das preferências: o motor de desempenho selecionado para o perfil com as opções padrão mais os parâmetros do kernel, e a opção de modo de energia do próprio cliente. O operador não definiu nada no braço c.

| Braço | Voltas (FPS) | Média |
|---|---|---:|
| o · o operador aplica os ajustes | 345,18 · 335,13 · 354,77 · 334,88 | 342,5 |
| c · o cliente v0.23.0 aplica os ajustes | 310,55 · 359,95 · 346,21 · 350,43 | 341,8 |

O log do motor e os processos em execução mostraram as mesmas opções ativas nos quatro boots. O cliente passa `-feature VulkanNativeSwapchain` como argumento onde o operador usou a variável de ambiente equivalente. Depois da série, o `config.ini` do perfil de teste voltou aos bytes originais. Outros programas da área de trabalho estavam abertos, então o nível de cerca de 342 FPS reflete essa condição do dia, descrita acima em carga em segundo plano, e não os números da área de trabalho ociosa. A série mostra que o caminho do cliente reproduz o caminho do operador nas mesmas condições. À parte, numa sessão normal (sem elevação) com um perfil novo e esta compilação do motor, o status de início do cliente confirmou as sete opções padrão, e a confirmação do boot não atrasou (18 s).

### Cipó Livre na Radeon 780M integrada

A mesma receita também rodou com o Cipó Livre (Godot, Vulkan) na Radeon 780M integrada, com a mesma imagem Android 17 Google Play, tela Android a 60 Hz e o modo Equilibrado do Windows. A janela Rust era uma compilação local do cliente v0.22.0. Seis boots a frio alternaram na ordem s, r, u, u, r, s, com uma amostra de 20 segundos por cena e por boot. A métrica conta os carimbos de tempo distintos de apresentação do SurfaceFlinger na superfície do jogo. A configuração publicada (s) usou o motor com cache da v0.22.0, o pipe goldfish, composição OpenGL e a opção da GPU integrada AMD. A receita (r) usou o motor de desempenho medido. O terceiro braço (u) acrescentou à receita a opção AMD (`-VulkanAllocateDeviceMemoryOnly`).

| Configuração | Cena | Apresentações/s | Intervalos > 1,5 período | P99 médio |
|---|---|---:|---:|---:|
| Publicada (s) | Floresta | 58,61 | 4,86% | 31,2 ms |
| Publicada (s) | Andando | 57,38 | 9,15% | 33,8 ms |
| Receita (r) | Floresta | 59,93 | 0,17% | 19,7 ms |
| Receita (r) | Andando | 59,89 | 0,08% | 19,4 ms |
| Receita + opção AMD (u) | Floresta | 60,01 | 0,00% | 19,2 ms |
| Receita + opção AMD (u) | Andando | 59,99 | 0,08% | 20,6 ms |

Os seis boots desenharam o jogo corretamente, e a opção AMD funcionou junto com `VulkanNativeSwapchain`. Cada valor reúne duas amostras de 20 segundos de dois boots. A tela de 60 Hz limita a taxa, e o jogo ajusta sozinho a resolução de renderização, então estes números mostram entrega mais regular, não mais trabalho de renderização. [Testes anteriores do Cipó Livre](../benchmarks/cipo-livre/README.md)

**Limites.** SuperTuxKart na RTX 5070 Ti e Cipó Livre na Radeon 780M, ambos no mesmo PC; o PUBG ainda não foi testado com este motor. O motor emite um aviso de que o GuestAngle é instável em níveis de API acima de 35. A espera de 3000 µs custa cerca de um núcleo extra do host enquanto o jogo roda. Com ela, cerca de uma volta em doze ficou fora da curva, perto de 300 FPS, e programas em segundo plano reduzem o resultado, como descrito acima. Com `kvm_pv_eoi`, o convidado identifica o hipervisor como KVM. No SuperTuxKart o Android continuou apresentando na taxa da tela de 90 Hz; esses números medem a velocidade com que o jogo renderiza. Sessões longas e snapshots ainda não foram testados com este motor.

## SuperTuxKart do primeiro protótipo até a v0.21.1

Com o cache Vulkan ativo nas quatro corridas da RTX, sincronizar o relógio virtual reduziu o uso de CPU do QEMU em **18,7%**, de 2.188 para 1.778 ms de CPU por segundo. O jogo marcou **90,17 FPS com o relógio padrão** e **90,02 com sincronização**. Os intervalos de P99 do Android se sobrepuseram. As duas configurações chegaram perto do teto de 90 Hz em FIFO. [Ensaio combinado de cache e relógio](../benchmarks/stk-1.5/windows-cache-tsc-kart-rtx.md)

Um segundo teste do motor Windows levou o jogo no Android 17 a **90,19 FPS** nas duas corridas com cache Vulkan. Com o cache desligado, o mesmo motor entregou **85,56 FPS** nas duas corridas de controle. O Android passou a apresentar **89,98 quadros/s**. Nos intervalos medidos, os quadros que demoraram mais de 16,667 ms passaram de **205 em 3.396** para **zero em 3.601**. Mantivemos a correção de relógio desligada. O P99 do BlueStacks continuou menor, em 11,11 ms, contra 14,53 a 14,63 ms no Rust experimental. [Quatro corridas, tempos de quadro e condições](../benchmarks/stk-1.5/windows-cached-memory-kart.md)

Mesmo PC com Windows, Ryzen 9 8945HS e GeForce RTX 5070 Ti. A pista é o replay automático Black Forest do SuperTuxKart 1.5, com Android renderizando a 1600 × 900. Os valores são **FPS calculados pelo jogo**, a partir do número de quadros e da duração integral da corrida.

| Teste na RTX 5070 Ti | RustEmuDroid | Kemu 0.7.1 | BlueStacks 5.22 Pie64 | Corridas por player |
|---|---:|---:|---:|---|
| 23/11 · Primeiro protótipo v0.1.0, Original, OpenGL ES, alvo 60 | 45,86 | 59,69 | Sem corrida concluída | 1 |
| 23/11 · Painel integrado v0.1.0, motor Kemu, Original, OpenGL ES, alvo 60 | 59,98 | 59,90 | — | 3 |
| 23/11 · Primeira rodada dos três, Original, OpenGL ES, alvo 60 | 59,93 | 59,98 | 60,02 | 3 |
| 23/11 · Mesma rodada, **Alta**, OpenGL ES, alvo 60 | 28,90 | 28,68 | 59,99 | 3 |
| 25/11 · v0.18.1, Alta, Vulkan FIFO, Android a 90 Hz, mesmo APK x86-64 | 82,85 | 90,02 | 89,99 | 2 |

No perfil Original a 60, a mudança de configuração do protótipo para o painel integrado rendeu **14,12 FPS (+30,8%)**. Também mudou o motor e o caminho OpenGL. Na primeira comparação em Alta, Rust e Kemu executavam Android 16/x86-64 via ANGLE. O BlueStacks executava Android 9/ARM de 32 bits. Na comparação a 90 Hz, os três executaram exatamente o mesmo APK x86-64 e registraram os mesmos 21 parâmetros gráficos. As versões de Android e os drivers de cada player continuaram diferentes. [Testes iniciais e arquivos por corrida](../benchmarks/stk-1.5/README.md) · [Seis corridas a 90 Hz](../benchmarks/stk-1.5/vulkan-end-scene.md#windows-results-at-90-hz)

O perfil gráfico também explica como foi possível ver **perto de 30 FPS** no kart. Em Alta, o Android 17 do Rust marcou **14,00** com o tradutor OpenGL ES padrão, **25,36** com ANGLE e **56,15** com Vulkan direto naquele ensaio de 23/11. Nas versões v0.2.4 a v0.7.0, o mesmo replay em Alta com Vulkan já sustentava **59,94 a 60,03 FPS** com alvo de 60. Essas corridas preservam o contexto da observação de 24–30 FPS durante o jogo. [Experimento de renderização](../benchmarks/stk-1.5/README.md#home-do-android-e-imagem-google-play) · [Comparação entre versões](game-testing.md#supertuxkart-release-comparison)

## Comparação dos três emuladores a 90 Hz com a v0.21.1

| Player e Android | FPS do jogo | Apresentações do Android/s | P99 por corrida (ms) | Área do jogo na janela Windows |
|---|---:|---:|---:|---:|
| RustEmuDroid v0.21.1 · Android 17 | **85,62** | 85,24 | 23,51 / 23,24 | 800 × 450 px |
| Kemu Qt 0.7.1 · Android 16 | **90,02** | 89,97 | 13,56 / 13,34 | 1280 × 720 px |
| BlueStacks 5.22.166.1003 · Android 9 | **89,99** | 89,98 | 11,11 / 11,11 | 1600 × 900 px |

O perfil habitual do Rust com Android 17 ficou **4,37 FPS (4,85%) atrás do BlueStacks** no jogo. Frente aos 82,85 FPS da v0.18.1, a diferença observada foi de **+2,77 FPS**. A v0.21.1 usou vídeo ajustado a 800 × 450, enquanto o teste da v0.18.1 transmitia 1600 × 900. O jogo continuou renderizando a 1600 × 900 nas duas versões. O painel Rust processou **82,69 quadros/s** na rodada atual.

Cada player completou duas corridas novas, um de cada vez, com quatro vCPUs, o mesmo APK x86-64, os mesmos 21 parâmetros em Alta, Vulkan FIFO e 90 Hz confirmados no Android. O BlueStacks usou seu player oficial e um clone da instância Pie64. Os tamanhos das janelas de exibição, o aquecimento e o instante de amostragem do Android diferem entre players. Com duas corridas por player, pequenas variações na média têm pouca força para estabelecer um vencedor. [Seis corridas, condições e dados](../benchmarks/stk-1.5/windows-current-three-players.md)

Os contadores respondem a perguntas diferentes. **FPS do jogo** inclui o replay completo. **Apresentações do Android/s** conta quadros novos da superfície do jogo numa janela de aproximadamente 20 segundos. O **P99 do intervalo** indica o tempo dentro do qual ocorreram 99% dos intervalos de apresentação nessa janela. Por exemplo, 22 ms são quase dois períodos de uma tela a 90 Hz. Medir o que chega ao monitor físico exigiria outra medição.

Nos dois trechos do Android somados por player, **192 de 3.411** intervalos no Rust, **0 de 3.600** no Kemu e **1 de 3.601** no BlueStacks passaram de 1,5 período a 90 Hz (16,667 ms, antes do arredondamento). No cronômetro separado do jogo, a média do trecho `drawAll() for kart 0` nas iterações acima desse limiar ficou **7,70 e 8,08 ms maior** nos dois replays do Rust. O cronômetro mede tempo decorrido, incluindo espera e interrupções de escalonamento. Os intervalos do Android e os laços do jogo cobrem janelas diferentes. [Contagens, escopos e traços das seis corridas](../benchmarks/stk-1.5/windows-current-three-players.md#where-the-longer-game-frames-spent-time)

## O que explica a diferença no kart?

Em um diagnóstico separado de Black Forest no Android 17 com a RTX 5070 Ti, os registros de objetos responderam por **96,295%** dos bytes gravados pela CPU nos buffers Vulkan instrumentados ao longo de 3.419 quadros. Os dados de uniformes e os comandos indiretos somaram **2,688%**, com média de **5.769 bytes por quadro**. O ensaio mediu volume. O tempo gasto em cada buffer segue em aberto, e essas voltas não entram na comparação de FPS. [Distribuição por quadro, dados brutos e patch](../benchmarks/stk-1.5/windows-vulkan-draw-bytes.md)

Com a mesma janela Rust, a mesma RTX e o mesmo APK, a função do kart que prepara os comandos de desenho Vulkan consumiu em média **3,47 ms de CPU por quadro nas duas voltas com SDK/Android 17**, frente a **0,55 e 0,54 ms com Kemu/Android 16**. A diferença foi de cerca de **2,9 ms por quadro** nessa etapa. No Android 17, ela chegou a **10,01 e 9,66 ms** nos 148 e 144 quadros longos das duas voltas. No Kemu, apenas 3 e 1 quadros passaram do limiar. Com a medição detalhada desligada e ligada, respectivamente, o jogo marcou **85,33 e 85,02 FPS** no SDK e **90,02 e 90,03 FPS** no Kemu. As oito voltas têm relatório completo do jogo. Na última volta do SDK a janela perdeu o foco antes da amostra separada de apresentações do Android. [Dados por volta e por etapa](../benchmarks/stk-1.5/windows-draw-all-android17.md)

- **Interface Rust.** Com o mesmo motor QEMU/gfxstream, a mesma imagem Android 16, o mesmo APK e a mesma RTX, RustEmuDroid v0.20.0 e Kemu Qt entregaram **90,02 FPS** cada. O Android apresentou **89,98** e **90,07 quadros/s**, respectivamente. O painel Rust processou **84,07 quadros/s**. Há margem para melhorar a exibição mesmo quando o jogo já alcança 90. No protótipo GPUI, a comparação de imagens compostas pelo Windows produziu 86,00 mudanças/s com GPUI e 84,14 com Glow. A diferença foi menor que a variação entre as duas passagens do GPUI. [Mesmo motor, duas interfaces](../benchmarks/stk-1.5/vulkan-end-scene.md#how-the-kemu-and-rust-windows-receive-game-frames) · [Teste do GPUI](../experiments/results/windows-viewers-stk-dxgi.md)
- **GPU.** Com a janela Rust v0.21.1 nos dois testes e a mesma RTX, o trecho instrumentado da fila Vulkan teve P95 de **0,7065 ms** no Android 17 do SDK e **0,6997 ms** no Android 16 do Kemu. Os intervalos de apresentação do Android tiveram P99 de **22,97 e 13,18 ms**. Partes do quadro fora do trecho instrumentado incluem aquisição da imagem, sincronização e apresentação. As evidências atuais dão mais motivo para investigar essas etapas e o relógio virtual do que para escrever um driver de GPU. [Tempos da fila gráfica e dos quadros](../benchmarks/stk-1.5/windows-vulkan-gpu-timestamps.md)
- **Imagem e relógio virtual.** Mantendo o motor do SDK e a janela Rust, medimos **83,67 FPS** no Android 16 Google APIs e **85,61** no Android 17 Google Play. O kernel do Android 17 detectou contadores dessincronizados entre as vCPUs e escolheu **HPET**. O Kemu manteve o **TSC**. Após o boot, uma sonda separada em repouso estimou uma diferença persistente de **1,877 s** entre os contadores TSC das quatro vCPUs do SDK. A inversão da ordem de leitura limitou os desvios do Kemu em relação à vCPU 0 a **0,26 ms**. Após o replay, a mesma leitura de `gettimeofday()` custou **8,44 µs** no SDK e **0,018 µs** no Kemu. No teste posterior que mudou apenas o relógio, o FPS do jogo subiu 2,11% na janela Rust. [Imagens oficiais no mesmo motor](../benchmarks/stk-1.5/windows-sdk-android16-vs17.md) · [Logs de boot](../benchmarks/stk-1.5/windows-guest-clocksource.md) · [Sincronização dos contadores](../benchmarks/stk-1.5/windows-guest-clock-alignment.md) · [Custo da leitura do relógio](../benchmarks/stk-1.5/windows-guest-clock-cost.md)
- **Custo do profiler do jogo.** Em uma rodada A/B/B/A separada, os dois motores usaram a mesma janela do Rust v0.21.1 a 800 × 450 px, com Black Forest em Alta, Vulkan FIFO, 1600 × 900 e 90 Hz no Android. Com o mesmo APK de teste, trocar a medição detalhada dos trechos de CPU pela contagem e duração dos quadros mudou o FPS agregado de **85,38 para 85,93** no SDK Android 17 e de **90,02 para 90,01** no Kemu Android 16. O ganho de 0,55 FPS no SDK ficou abaixo do critério definido para o ensaio. O Kemu ainda entregou **4,08 FPS** a mais com a contagem de quadros. O FPS do jogo cobre a corrida inteira. A medição do SurfaceView do Android cobre uma amostra separada de 20 segundos. A rodada oficial dos três players acima inclui o BlueStacks e a janela original do Kemu. [Oito replays e critérios do ensaio](../benchmarks/stk-1.5/windows-profiler-overhead.md)
- **CPU.** Na v0.21.1, uma configuração experimental que ativa composição Vulkan e ANGLE reduziu em **14,03%** o tempo de CPU de QEMU por apresentação do Android. O kart passou de **85,61 para 85,77 FPS**. A economia de CPU foi o benefício verificável nessa carga. [Quatro corridas na mesma imagem Android 17](../benchmarks/stk-1.5/windows-android17-vulkan-composition.md)

Uma compilação experimental do QEMU para Windows já inicia a mesma imagem Google Play com os contadores das CPUs virtuais sincronizados. Ao ativar a correção no mesmo executável, o Android passou de HPET para TSC. O limite medido para a diferença entre os contadores caiu de 2,258 segundos para 0,252 milissegundo. Uma leitura de hora pela libc caiu de 9.114 para 18 ns de CPU. Em quatro corridas na janela Qt do motor, o kart passou de **85,45 para 87,38 FPS (+2,26%)**, e a proporção de intervalos longos na apresentação do Android caiu de 5,32% para 3,41%. A comparação dos três emuladores usa a v0.21.1 publicada. [Validação do relógio](../benchmarks/stk-1.5/whpx-tsc-sync.md#android-17-cold-boots-on-the-same-executable) · [Resultados da corrida experimental](../benchmarks/stk-1.5/windows-whpx-clock-kart.md)

## Radeon 780M integrada, PUBG e memória

| Radeon 780M · Alto em Vulkan, 90 Hz | FPS do jogo | Quadros/s do Android | P99 do Android por corrida |
|---|---:|---:|---:|
| RustEmuDroid · Android 17, cache ligado | 90,17 | 89,95 | 12,88 / 13,34 ms |
| Kemu Qt · Android 16 | 90,02 | 90,09 | 12,78 / 13,02 ms |
| BlueStacks · Android 9 Pie64 | 89,72 | 89,79 | 11,11 / 11,11 ms |

Os três chegaram perto do alvo de 90 Hz com o mesmo APK e os mesmos ajustes de renderização no jogo. Cada linha reúne duas corridas completas. No Windows, a imagem ocupou 800 × 450 no painel Rust, 1280 × 720 no Kemu e 1600 × 900 no BlueStacks. [Medições por corrida, intervalos de amostragem e dados brutos](../benchmarks/stk-1.5/windows-current-three-players-radeon780m.md)

Na **Radeon 780M integrada a 90 Hz**, a mesma janela Rust com Android 17 e o mesmo QEMU experimental passaram de **84,36 FPS com o cache desligado para 90,17 FPS com ele ligado (+6,89%)**. O P99 do intervalo entre quadros do Android caiu de **23,10–23,22 ms para 12,88–13,34 ms**. O painel Rust processou 86,72 quadros/s com o cache ligado. Os contadores do Windows confirmaram atividade na Radeon nas quatro corridas. A v0.22.0 para Windows permite selecionar esse motor por perfil. [Condições, quatro corridas e dados de cada quadro](../benchmarks/stk-1.5/windows-cached-memory-radeon780m.md) · [Como configurar na v0.22.0](releases/v0.22.0.md)

| Medição | RustEmuDroid | Kemu | BlueStacks |
|---|---:|---:|---:|
| Kart, Radeon 780M, Alta, OpenGL ES, alvo 60 · FPS do jogo | 55,49 | 59,87 | 59,99 |
| Kart, Radeon 780M, Alta, Vulkan, alvo 60 · FPS do jogo | 59,71 | — | — |
| PUBG, RTX, lobby e inventário, Balanced/High · apresentações/s | 25,20 | — | 29,58 |
| PUBG, RTX, campo de treino com movimento de câmera, Smooth/High · apresentações/s | 29,29 | — | 29,98 |
| Memória residente do processo da VM na Home do Android · GiB | 5,73 | 3,04 | 1,67 |

Na integrada, foram três corridas por configuração. A linha OpenGL ES usa Android 16 para Rust e Kemu e Android 9 para BlueStacks. A linha Vulkan usa Android 17 e filtragem anisotrópica 16×, frente a 8× no OpenGL ES. A primeira medição do protótipo foi na RTX. [Dados da Radeon 780M](../benchmarks/stk-1.5/radeon-780m/README.md)

No PUBG, o problema mais perceptível continua sendo **a pausa ao fechar o inventário pela primeira vez**. Levou **1,60–1,68 s** no Rust v0.18.0 e **0,17 s** no BlueStacks. A v0.21.1 ainda demorou **1,61–1,65 s** em três execuções, embora o lobby parado apresentasse 29,37 quadros/s. No treino, o BlueStacks chegou a 59,89 apresentações/s ao selecionar *Extreme*. O maior ajuste oferecido ao Android 17 testado foi *High*. Falta uma série do Kemu nesse percurso. [Comparação no PUBG](game-testing.md#fps-comparisons-on-the-windows-test-pc) · [Reteste na v0.21.1](../benchmarks/pubg-mobile/v0.21.1-android17-first-close-meminfo.md)

O Kemu Qt 0.7.1, em uma cópia isolada do Android 16, iniciou o PUBG 4.7.0/21718 ARM64 com a RTX 5070 Ti e tela a 60 Hz. O processo caiu com `SIGILL` no modo padrão e nas duas tentativas com tradução leve. No modo leve chegou à `GameActivity` e apareceu brevemente o splash do Android, sem chegar ao Lobby. Um registro histórico separado usou o launcher local do Kemu, o build **21783** e o tutorial 3D em Balanced. Contou **1.812 apresentações em 30,19 s (60,02/s)**, com P99 de **19,32 ms**. As diferenças de build, cena e launcher, além de BlueStacks aberto no ensaio antigo, impedem usar o tutorial como resultado Kemu de Lobby, Inventário ou Treino. [Tentativa nova e método da amostra histórica](../benchmarks/pubg-mobile/windows-kemu-qt-21718.md)

As três VMs tinham 8 GiB configurados. A memória da tabela é a mediana de três leituras do processo principal de cada VM no Windows. A interface Rust usou outros **118 MiB**. A idade das sessões e as Homes diferiram, o que limita a comparação de consumo. O relatório cobre este PC Windows. Jogos, GPUs e versões diferentes exigem medições próprias. [Contadores de memória e horários](../benchmarks/windows-home-memory/README.md)
