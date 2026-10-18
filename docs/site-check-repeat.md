# Site check

Made by `tools/site_check.py`. One site per fresh headless Lumen, release build, signed out.

| Site | Blocker | Works | Load | Lag (long tasks) | Worst | Memory | Idle CPU | Errors | Blocked |
|---|---|---|---|---|---|---|---|---|---|
| youtube | on | yes (video at 8.5s) | 2126 ms | 26 = 3306 ms | 538 ms | 743 MB | 8.17% | 30 | 16 |
| youtube | on | yes (video at 10.4s) | 2154 ms | 14 = 2070 ms | 556 ms | 710 MB | 6.96% | 10 | 35 |
| youtube | on | yes (video at 8.0s) | 2561 ms | 21 = 2864 ms | 514 ms | 784 MB | 8.01% | 6 | 18 |
| youtube | off | yes (video at 10.7s) | 1816 ms | 11 = 1936 ms | 537 ms | 726 MB | 3.29% | 6 | 0 |
| youtube | off | yes (video at 10.0s) | 2042 ms | 11 = 1846 ms | 515 ms | 778 MB | 2.87% | 12 | 0 |
| youtube | off | yes (video at 10.0s) | 2302 ms | 19 = 2668 ms | 672 ms | 767 MB | 3.52% | 8 | 0 |
| youtube-search | on | yes (17 results) | 1565 ms | 10 = 1503 ms | 650 ms | 576 MB | 0.39% | 28 | 10 |
| youtube-search | on | yes (17 results) | 1576 ms | 13 = 1770 ms | 651 ms | 569 MB | 0.43% | 28 | 8 |
| youtube-search | on | yes (14 results) | 1599 ms | 9 = 1446 ms | 650 ms | 565 MB | 0.14% | 4 | 10 |
| youtube-search | off | yes (15 results) | 1499 ms | 11 = 1541 ms | 632 ms | 562 MB | 0.02% | 12 | 0 |
| youtube-search | off | yes (13 results) | 1559 ms | 8 = 1266 ms | 584 ms | 549 MB | 0.14% | 0 | 0 |
| youtube-search | off | yes (13 results) | 1486 ms | 9 = 1260 ms | 572 ms | 527 MB | 0.1% | 0 | 0 |
| github | on | yes (readme) | 1359 ms | 2 = 203 ms | 143 ms | 249 MB | 0.01% | 1 | 5 |
| github | on | yes (readme) | 966 ms | 5 = 504 ms | 195 ms | 266 MB | 0.01% | 1 | 4 |
| github | on | yes (readme) | 962 ms | 4 = 425 ms | 198 ms | 259 MB | 0.04% | 1 | 4 |
| github | off | yes (readme) | 1055 ms | 2 = 198 ms | 141 ms | 244 MB | 0.02% | 1 | 0 |
| github | off | yes (readme) | 1405 ms | 2 = 120 ms | 69 ms | 254 MB | 0.03% | 1 | 0 |
| github | off | yes (readme) | 2549 ms | 2 = 200 ms | 150 ms | 249 MB | 0.0% | 1 | 0 |
| spotify | on | yes (2359 chars) | 2102 ms | 4 = 431 ms | 196 ms | 495 MB | 0.9% | 29 | 3 |
| spotify | on | yes (2451 chars) | 2158 ms | 4 = 426 ms | 201 ms | 498 MB | 0.98% | 29 | 3 |
| spotify | on | yes (2451 chars) | 2125 ms | 4 = 403 ms | 196 ms | 494 MB | 0.75% | 29 | 3 |
| spotify | off | yes (2702 chars) | 2067 ms | 6 = 518 ms | 188 ms | 535 MB | 0.89% | 1 | 0 |
| spotify | off | yes (2702 chars) | 2153 ms | 6 = 625 ms | 241 ms | 540 MB | 0.91% | 1 | 0 |
| spotify | off | yes (2702 chars) | 2004 ms | 6 = 553 ms | 188 ms | 537 MB | 1.04% | 1 | 0 |
