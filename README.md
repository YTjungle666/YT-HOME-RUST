# YT HOME RUST

![Release](https://img.shields.io/github/v/release/YTjungle666/YT-HOME-RUST?display_name=tag)
![CI](https://img.shields.io/github/actions/workflow/status/YTjungle666/YT-HOME-RUST/ci.yml?branch=main&label=ci)
![Docker](https://img.shields.io/github/actions/workflow/status/YTjungle666/YT-HOME-RUST/docker.yml?branch=main&label=docker)
![License](https://img.shields.io/github/license/YTjungle666/YT-HOME-RUST)

`YT HOME RUST` 是一个面向家庭网络回家场景的 `sing-box` 控制面板。  
它把入站、客户端、二维码、订阅、TLS / Reality、运行状态和访问边界统一到一个中文面板里，适合部署在家庭服务器、`PVE`、`NAS` 和小主机上。

## 它解决什么问题

- 不再手工拼回家节点配置
- 用一个面板管理入站、客户端、订阅和二维码
- 明确区分普通公网访问节点与“代理回家”节点
- 让手机、平板、电脑快速导入并稳定使用

## 你会得到什么

- 简体中文界面
- 适合家庭回家场景的 `sing-box` 控制面
- 可直接导入客户端的订阅、链接与二维码
- 默认收紧的访问边界控制
- Rust 后端带来的更稳资源占用和更清晰的结构

## 默认信息

- 面板地址：`http://<你的地址>/`
- 订阅地址：`http://<你的地址>:2096/sub/`
- 默认账号：`admin`
- 默认密码：`admin`
- 当前发布平台：`linux/amd64`

## 部署方式 1：一键安装

适合已经有 Linux 主机，希望几分钟内装好就开始用。

直接安装最新版：

```bash
bash <(curl -Ls https://raw.githubusercontent.com/YTjungle666/YT-HOME-RUST/main/install.sh)
```

安装指定版本：

```bash
bash <(curl -Ls https://raw.githubusercontent.com/YTjungle666/YT-HOME-RUST/main/install.sh) v2.0.0
```

说明：

- 支持 `systemd` 和 `OpenRC`
- Alpine 请先保证系统里有 `bash`
- 安装完成后直接访问面板地址即可

## 部署方式 2：Docker / GHCR 镜像

适合已经用 Docker 管理服务的环境。

镜像地址：

```text
ghcr.io/ytjungle666/yt-home-rust
```

直接运行：

```bash
docker run -d \
  --name yt-home-rust \
  --restart unless-stopped \
  -p 80:80 \
  -p 2096:2096 \
  -v $(pwd)/db:/app/db \
  ghcr.io/ytjungle666/yt-home-rust:latest
```

使用 Compose：

```bash
mkdir -p yt-home-rust && cd yt-home-rust
curl -LO https://raw.githubusercontent.com/YTjungle666/YT-HOME-RUST/main/docker-compose.yml
docker compose up -d
```

## 部署方式 3：PVE CT 模板

适合已经习惯在 `PVE LXC/CT` 里直接跑服务的环境。

Release 页面会直接提供可创建 CT 的 rootfs 包：

```text
yt-home-rust-ct-amd64-rootfs.tar.gz
```

创建示例：

```bash
pct create 210 local:vztmpl/yt-home-rust-ct-amd64-rootfs.tar.gz \
  --hostname yt-home-rust \
  --cores 2 \
  --memory 1024 \
  --rootfs local-lvm:8 \
  --net0 name=eth0,bridge=vmbr0,ip=dhcp
```

启动：

```bash
pct start 210
```

这个 rootfs 已经内置 CT 启动入口，创建后可直接启动，不依赖额外容器运行时。

## 发布产物

- Linux 安装包：`s-ui-linux-amd64.tar.gz`
- PVE CT rootfs：`yt-home-rust-ct-amd64-rootfs.tar.gz`
- Docker 镜像：`ghcr.io/ytjungle666/yt-home-rust`
- Release 页面：<https://github.com/YTjungle666/YT-HOME-RUST/releases>

## 使用建议

- 面板部署在内网服务器，公网只开放必要端口
- 只有确实需要回家访问的入站才开启“代理回家”
- Reality 节点建议使用你自己可控的域名
- 首次登录后按自己的环境修改面板端口、订阅地址和域名

## 许可证

`GPL-3.0-only`
