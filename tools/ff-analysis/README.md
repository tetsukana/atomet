# FF 検出の解析スクリプト（研究用・暫定）

FF ブロック（`FF_*.fits` と同名の `.json` サイドコー）を、検出結果の確認・ラベル付け・可視化に使う
Python スクリプト。研究計画は `docs/ff-detection-research.md` を参照。

> 暫定版: 一部にパスがハードコードされている（`sky/ff_showers.py` / `sky/ff_skyview.py` の `--csv` 既定値、
> `sky/ff_ds9.py` の DS9 パス `C:\SAOImageDS9\ds9.exe`）。使うときは引数かコードで合わせること。
> 研究が進んだら共有クレート＋オフラインCLI（ロードマップ1）に置き換える。

## フォルダ
| フォルダ | 役割 |
|---|---|
| `offline/` | 保存ブロックの走査・一晩の集計・目視レビュー（`ff_offline.py` を `ff_night.py` と `ff_event_sheet.py` が import するので、同じフォルダに置く） |
| `detect/` | `ff_redetect` / `ff_inject`（Rust）の出力の図（タイル図、サンソン図法、検出率、BCSH のノイズ） |
| `video/` | 検出時刻の動画の切り出し・結合、検出位置の一覧画像 |
| `sky/` | 流星群の判定・天球への重ね描き・DS9・ソルバー向けの書き出し |

`ff_redetect.rs` と `ff_inject.rs`（`crates/solver/examples/`）は、検出器 `crates/atometd/src/ff_detect.rs` を取り込む研究用の
Rust ツール。ローカルだけに置き、git には入れていない（`.gitignore`）。結果は `docs/results/` にある。

必要: Python 3, `numpy`, `astropy`, `matplotlib`, `scipy`（任意）, `ffmpeg`（`video/ff_clips.py`）。
Windows では日本語環境でも動くように、データ側のパスは `D:/ff/...` のようにスラッシュで渡す。

| スクリプト | 内容 | 例 |
|---|---|---|
| `offline/summarize.py` | サイドコーの `ff_meteors` / `realtime_events` を表にする | `python offline/summarize.py D:/ff/detections/2026/11/14 01 02 03` |
| `video/montage.py` | 検出位置の `maxpixel−avepixel` を切り出して一覧画像に | `python video/montage.py <dir> out.png 01 02` |
| `offline/ff_review.py` | 1ブロックずつ表示し、`g`=本物 / `x`=誤検出 を `review.csv` に記録 | `python offline/ff_review.py <dir> 01 02` |
| `sky/ff_showers.py` | オフライン流星群判定、地図（RA/Dec・北極図）、偶然一致のモンテカルロ | `python sky/ff_showers.py <dir> 01 02 --tz 9` |
| `sky/ff_skyview.py` | カメラ画像上に全流星・後方延長・放射点を重ね描き（FITS の WCS 使用） | `python sky/ff_skyview.py <dir> 01 02` |
| `offline/ff_composite.py` | `review.csv` の good ブロックの比較明合成 | `python offline/ff_composite.py <dir> 01 02` |
| `video/ff_clips.py` | FF検出時刻から常時録画（1分ファイル）を切り出す | `python video/ff_clips.py <ff dir> <records dir> 01 02` |
| `offline/ff_offline.py` | `ff.rs` の検出(`ff_detect_meteor`)のPython移植。全ゲートがパラメータで、棄却理由付き。`validate`=サイドコーと一致確認 / `scan`=保存ブロックを走査（`--relaxed` で緩い閾値）/ `sheet`=候補のコンタクトシート | `python offline/ff_offline.py scan D:/ff/2026/11/14 00 01 02 --relaxed --out cand.jsonl` |
| `offline/ff_night.py` | 一晩分: 段階別の検出数（時間・太陽高度付き）＋ 流星群判定（放射点が地平線上にあること）＋ 図（時間推移・群別・北極図）・`events.csv`。`--c-corr` `--c-speed` `--one-per-block` で絞り込み | `python offline/ff_night.py cache.jsonl --from 20261118_180000 --to 20261119_014240 --out DIR` |
| `offline/ff_night_composite.py` | 一晩の流星の比較明合成（全体・流星だけ・流星群ごとに色分け）。`--exclude` で目視で偽と判断したブロックを除く | `python offline/ff_night_composite.py events.csv cache.jsonl --exclude 040635_605 --out DIR` |
| `offline/ff_event_sheet.py` | 段階別のイベントをコンタクトシートにして目視（ページ送りは index） | `python offline/ff_event_sheet.py C out.png 0 36 <cacheのdir> 20261118_180000` |
| `detect/ff_redetect_tiles.py` | `ff_redetect --dump` の jsonl のイベント（段階A→B→C）をタイル図に。番号 N は次の `INSECT` で使う | `python detect/ff_redetect_tiles.py redetect.jsonl out.png D:/ff/detections/2026/11/21 D:/ff/2026/11/21` |
| `detect/ff_redetect_sanson.py` | 同じ jsonl をサンソン図法（正弦曲線図法）に描く: 後方延長した大圏、活動中の放射点、群判定。環境変数 `INSECT=16,17`（除く N 番号）、`CSV`、`TZ_HOURS`。1晩分だけ | `INSECT=16,17 python detect/ff_redetect_sanson.py redetect.jsonl out.png <fits root>...` |
| `detect/ff_bcsh_scan.py` | ブロックごとの空のノイズ（stdpixel、閾値超えの画素の割合）を、時間順に出す。ISP の設定（BCSH オン/オフ）が変わった時刻を探す。`docs/ff-detection-improvement-20261120.md` §9 | `python detect/ff_bcsh_scan.py out.json 20261121_000000 20261121_060000 3 D:/ff/2026/11/21` |
| `video/ff_join_clips.py` | `video/ff_clips_cand.py` の印付きクリップを、字幕付きで1本の動画に結合 | `python video/ff_join_clips.py <clips dir> out.mp4 --speed 1.0` |
| `sky/ff_solver_export.py` | FF ブロックの avepixel を生の 640x360 u8 で書き出す（`crates/solver/examples/baseline_solve.rs` 用）。ヘッダーの WCS の指す向きも一覧に | 使い方は冒頭の説明を参照 |
| `sky/ff_ds9.py` | DS9で4拡張を1ブロックずつ開く（起動確認は途中） | `python sky/ff_ds9.py <dir> 01` |

## 注意
- `block_start` はデバイスのローカル時刻。流星群判定は `--tz`（既定9=JST）で UTC に直している。
- `video/ff_clips.py` は録画ファイルが分頭から最大4秒遅れて始まる（GOP=4秒）ため、前後に余白（既定 5秒/4秒）を付けて切る。
- `review.csv` は FF ディレクトリ（日付ディレクトリ）直下に作られる。リポジトリには含めない。
- `sky/ff_showers.py` / `sky/ff_skyview.py` の判定は `crates/atometd/src/showers.rs` と同じ式の手書き再実装。
  Rust側を変えたらここも合わせるか、共有クレート化で一本化する。

## ff_inject (artificial meteors)
`cargo build --release -p solver --example ff_inject`; see the header of `crates/solver/examples/ff_inject.rs` and
`docs/ff-artificial-meteor-plan.md`. `--check` = bright-line sanity run; otherwise sweep A×L×v and write `--csv`,
plot with `python detect/ff_inject_plot.py in.csv out.png`.
