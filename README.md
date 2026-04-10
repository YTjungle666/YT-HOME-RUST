# YT HOME RUST

![Release](https://img.shields.io/github/v/release/YTjungle666/YT-HOME-RUST?display_name=tag)
![CI](https://img.shields.io/github/actions/workflow/status/YTjungle666/YT-HOME-RUST/ci.yml?branch=main&label=ci)
![Docker](https://img.shields.io/github/actions/workflow/status/YTjungle666/YT-HOME-RUST/docker.yml?branch=main&label=docker)
![License](https://img.shields.io/github/license/YTjungle666/YT-HOME-RUST)

`YT HOME RUST` 是一个面向家庭网络回家场景的 `sing-box` 控制面板。  
它把入站、客户端、二维码、订阅、TLS / Reality、运行状态和访问边界统一到一个中文面板里，适合部署在 `PVE`、`NAS`、小主机或家庭服务器上。

## 这是什么

如果你希望：

- 用一个面板统一管理回家节点，而不是手工拼配置
- 给手机、平板、电脑稳定下发二维码和订阅
- 区分“普通公网访问”和“代理回家访问”
- 在不改动习惯的前提下，把节点管理做得更稳、更清晰

那么 `YT HOME RUST` 就是这个产品。

## 你会得到什么

- 统一的中文管理界面
- 常见协议与 Reality 场景的集中配置能力
- 可直接导入客户端的二维码、链接与订阅
- 更明确的入站边界控制
- 默认账号即可启动，部署完成后就能进入面板

## 适合部署在哪里

- 家庭宽带回家机
- `PVE` 或 `LXC/CT`
- `NAS`
- 云服务器
- 低功耗小主机

## 默认信息

- 面板地址：`http://<你的地址>/`
- 订阅地址：`http://<你的地址>:2096/sub/`
- 默认账号：`admin`
- 默认密码：`admin`
- 支持架构：`amd64`、`arm64`

## 部署方式 1：一键安装脚本

适合已经有 Linux 主机，希望几分钟内装好就开始用。

直接安装最新版：

```bash
bash <(curl -Ls https://raw.githubusercontent.com/YTjungle666/YT-HOME-RUST/main/install.sh)
```

安装指定版本：

```bash
bash <(curl -Ls https://raw.githubusercontent.com/YTjungle666/YT-HOME-RUST/main/install.sh) v2.0.0
```

安装完成后访问：

- 面板：`http://你的服务器IP或域名/`
- 订阅：`http://你的服务器IP或域名:2096/sub/`

## 部署方式 2：Docker / GHCR 镜像

适合已经使用 Docker 或 Compose 管理服务的环境。

镜像发布到：

```text
ghcr.io/ytjungle666/yt-home-rust
```

### 直接运行

```bash
docker run -d \
  --name yt-home-rust \
  --restart unless-stopped \
  -p 80:80 \
  -p 2096:2096 \
  -v $(pwd)/db:/app/db \
  ghcr.io/ytjungle666/yt-home-rust:latest
```

### 使用 Compose

```bash
mkdir -p yt-home-rust && cd yt-home-rust
curl -LO https://raw.githubusercontent.com/YTjungle666/YT-HOME-RUST/main/docker-compose.yml
docker compose up -d
```

这个镜像已经按部署场景做了最小化收敛，既能直接作为 Docker 镜像运行，也能导出成 `PVE CT` 根文件系统。

## 部署方式 3：PVE CT 模板

适合已经习惯用 `PVE LXC/CT` 跑服务的环境。

可以直接把 Docker 镜像导出为 rootfs，再创建 CT。

### 1. 拉取镜像

```bash
docker pull ghcr.io/ytjungle666/yt-home-rust:latest
```

### 2. 导出 rootfs

```bash
cid=$(docker create ghcr.io/ytjungle666/yt-home-rust:latest)
docker export "$cid" | gzip > yt-home-rust-ct-rootfs.tar.gz
docker rm "$cid"
```

### 3. 上传到 PVE 宿主机

把 `yt-home-rust-ct-rootfs.tar.gz` 放到 PVE 宿主机，例如：

```bash
/var/lib/vz/template/cache/yt-home-rust-ct-rootfs.tar.gz
```

### 4. 创建 CT

```bash
pct create 210 local:vztmpl/yt-home-rust-ct-rootfs.tar.gz \
  --hostname yt-home-rust \
  --cores 2 \
  --memory 1024 \
  --rootfs local-lvm:8 \
  --net0 name=eth0,bridge=vmbr0,ip=dhcp
```

### 5. 启动 CT

```bash
pct start 210
```

镜像内已经内置 `CT init` 启动脚本，CT 启动后会自动拉起 `YT HOME RUST`。

## 发布产物

- Linux：`s-ui-linux-amd64.tar.gz`、`s-ui-linux-arm64.tar.gz`
- Windows：`s-ui-windows-amd64.zip`、`s-ui-windows-arm64.zip`
- Docker：`ghcr.io/ytjungle666/yt-home-rust`
- Release 页面：<https://github.com/YTjungle666/YT-HOME-RUST/releases>

## 使用建议

- 面板部署在内网服务器，公网只放必要端口
- 只把确实需要“回家访问”的入站开启“代理回家”
- Reality 节点建议使用你自己可控的域名
- 首次登录后，按自己的环境修改面板端口、订阅地址和域名

## 许可证

`GPL-3.0-only`
