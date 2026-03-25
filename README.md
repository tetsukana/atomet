# atomet

ATOM Cam2 + Meteor Station

ATOM Cam2 向けカスタム Linux ファームウェア。

## 構成

- **カーネル**: Linux 3.10.98 (Wyze/Ingenic カスタム、T31 SoC)
- **アーキテクチャ**: MIPS32 R2 LE (mipsel-ingenic-linux-gnu)
- **ビルドシステム**: Buildroot 2024.02
- **パッケージ**: BusyBox + wpa_supplicant + OpenSSH + NTP + ATBM WiFi ドライバ

## ネットワーク起動の優先順位

1. **USB-Ethernet アダプタ** (開発時に便利 — SDカード設定不要で接続できる)
2. **WiFi (ATBM603x)** — SDカードの `wpa_supplicant.conf` を読み込む

## インストール

1. ビルド成果物 (`factory_t31_ZMC6tiIDQN`, `rootfs_hack.squashfs`) をSDカードのルートに置く
2. 以下のファイルも SDカードに配置する

```
/factory_t31_ZMC6tiIDQN   ← カーネル (uImage.lzma をリネーム)
/rootfs_hack.squashfs      ← rootfs
/wpa_supplicant.conf       ← WiFi 設定
/authorized_keys           ← SSH 公開鍵
```

### wpa_supplicant.conf の例
```
ctrl_interface=/var/run/wpa_supplicant
network={
    ssid="MyWiFi"
    psk="MyPassword"
    scan_ssid=1
}
```

### SSH 公開鍵の生成
```bash
ssh-keygen -t ed25519 -f ~/.ssh/atomet
cat ~/.ssh/atomet.pub  # → SDカードの authorized_keys にコピー
```

### SSH 接続
```bash
ssh -i ~/.ssh/atomet root@atomet.local
```

rootパスワード: `atomet`

---

## ビルド方法

### 方法 1: Docker (推奨)

Docker Desktop (Windows/Mac) または Docker Engine (Linux) が必要。

```bash
# イメージをビルドしてコンテナを起動
docker compose build
docker compose up -d

# コンテナ内でビルド実行
docker compose exec builder docker_build

# 成果物は output/ に生成される
```

または一発で:
```bash
docker compose run --rm builder docker_build
```

### Rust デーモンのみビルド (開発用)
```bash
docker compose run --rm builder rust_build
scp output/atometd root@atomet.local:/media/mmc/
ssh root@atomet.local '/etc/init.d/S90atomet restart'
```

### 方法 2: WSL2 / Linux でローカルビルド

```bash
# 依存パッケージのインストール (Ubuntu/Debian)
sudo apt update
sudo apt install -y build-essential git wget curl unzip rsync bc cpio \
  python3 libssl-dev libelf-dev flex bison file zip lzop gawk \
  texinfo help2man libtool cmake autoconf

# ビルド実行
bash buildscripts/build_local.sh
```

### 方法 3: GitHub Actions

`main` または `build` ブランチに push すると自動ビルドされる。
成果物は Actions の Artifacts からダウンロードできる。

---

## 起動シーケンス

```
U-Boot
  └─ factory_t31_ZMC6tiIDQN (uImage.lzma = カーネル + initramfs)
       └─ initramfs /init
            ├─ SDカードをマウント (/media/mmc)
            ├─ rootfs_hack.squashfs をマウント → switch_root
            └─ BusyBox init (/sbin/init)
                 ├─ S20network  USB-Eth or WiFi
                 ├─ S40hostname ホスト名設定
                 ├─ S42ntpd     時刻同期 (ntp.nict.jp)
                 ├─ S50avahi    mDNS (atomet.local)
                 ├─ S80sshd     SSH サーバ
                 └─ S90atomet   atometd デーモン
```
