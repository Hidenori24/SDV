# ADR-0001: AUTOSAR-like Architecture採用

## Status
Accepted

## Context
SDVの“ソフトで車を定義する”要点を、個人開発でも破綻しない形で成立させる必要がある。  
部品分割、I/F固定、基盤サービス分離、監視と更新を組み込むには、AUTOSARの構造が有効。

## Decision
Classic AUTOSARを参考にした “AUTOSAR-like” を採用する。  
ただし、規格準拠やARXML生成は行わず、以下を最小要件とする。

- SWC（Runnable）分割
- RTE（Read/Write）で信号共有
- BSW（Time/Com/Nvm/Diag/Logging）を分離

## Consequences
- 拡張（UDP化、ECU分割、差し替え）に強い
- 初期の実装量は増えるが、後で破綻しにくい
