# Site check

Made by `tools/site_check.py`. One site per fresh headless Lumen, release build, signed out.

| Site | Blocker | Works | Load | Lag (long tasks) | Worst | Memory | Idle CPU | Errors | Blocked |
|---|---|---|---|---|---|---|---|---|---|
| youtube | on | yes (video at 8.8s) | 2222 ms | 18 = 2619 ms | 534 ms | 778 MB | 7.79% | 6 | 21 |
| youtube | on | yes (video at 8.7s) | 2017 ms | 17 = 2513 ms | 491 ms | 779 MB | 7.07% | 6 | 21 |
| youtube | off | yes (video at 10.7s) | 1736 ms | 12 = 1722 ms | 492 ms | 751 MB | 2.99% | 0 | 0 |
| youtube | off | yes (video at 10.6s) | 1885 ms | 13 = 2104 ms | 592 ms | 734 MB | 9.66% | 0 | 0 |
| youtube-search | on | yes (13 results) | 1328 ms | 10 = 1377 ms | 581 ms | 585 MB | 0.36% | 4 | 14 |
| youtube-search | on | yes (17 results) | 2022 ms | 20 = 3589 ms | 1056 ms | 589 MB | 0.21% | 28 | 6 |
| youtube-search | off | yes (13 results) | 8724 ms | 10 = 1400 ms | 605 ms | 552 MB | 0.19% | 6 | 0 |
| youtube-search | off | yes (17 results) | 1579 ms | 11 = 1582 ms | 683 ms | 577 MB | 0.06% | 0 | 0 |
| ytmusic | on | yes (30 items) | 2459 ms | 6 = 1250 ms | 498 ms | 387 MB | 0.09% | 2 | 4 |
| ytmusic | on | yes (59 items) | 2144 ms | 5 = 883 ms | 358 ms | 441 MB | 0.11% | 2 | 4 |
| ytmusic | off | yes (57 items) | 1829 ms | 4 = 799 ms | 341 ms | 420 MB | 0.09% | 0 | 0 |
| ytmusic | off | yes (30 items) | 1932 ms | 3 = 747 ms | 367 ms | 405 MB | 0.11% | 0 | 0 |
