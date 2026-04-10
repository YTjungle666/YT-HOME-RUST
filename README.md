# YT HOME RUST

![Release](https://img.shields.io/github/v/release/YTjungle666/YT-HOME-RUST?display_name=tag)
![CI](https://img.shields.io/github/actions/workflow/status/YTjungle666/YT-HOME-RUST/ci.yml?branch=main&label=ci)
![Docker](https://img.shields.io/github/actions/workflow/status/YTjungle666/YT-HOME-RUST/docker.yml?branch=main&label=docker)
![License](https://img.shields.io/github/license/YTjungle666/YT-HOME-RUST)

`YT HOME RUST` 是一个面向家庭网络回家场景的 `sing-box` 控制面板。  
它提供统一的入站、客户端、订阅、二维码、TLS/Reality 和状态管理界面，适合部署在 `PVE`、`NAS`、小主机或家庭服务器上，集中管理你的回家入口。

## 你会得到什么

- 把家庭网络回家入口集中管理，不再手工拼配置
- 用一个面板管理节点、客户端、订阅和 Reality 参数
- 让手机、平板、电脑通过二维码或订阅快速导入
- 区分普通公网节点和“代理回家”节点
- 默认收紧访问边界，未开启“代理回家”的入站不能访问服务器内网

## 核心能力

- Rust 后端，模块化重构
- 保留原有界面风格和核心交互
- 只保留简体中文界面资源
- 支持 `VLESS / VMess / Trojan / Hysteria / TUIC / Reality` 等常见场景
- 订阅、二维码、导入链接可直接给主流客户端使用
- GitHub 自动生成 `CI`、`Release` 和 `Docker` 镜像

## 默认信息

- 面板地址：`http://<你的地址>/`
- 订阅地址：`http://<你的地址>:2096/sub/`
- 默认账号：`admin`
- 默认密码：`admin`

## 部署方式 1：一键安装脚本

和原项目一样，保留一键安装方式。

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

这个镜像针对部署做了最小化收敛，既能直接当 Docker 镜像运行，也能导出成 `PVE CT` 根文件系统使用。

## 部署方式 3：PVE CT 模板

如果你习惯在 `PVE LXC/CT` 里运行服务，可以直接用 Docker 镜像导出 rootfs，然后创建 CT。

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

## 其他

如果你想看开发和贡献说明，请看 [CONTRIBUTING.md](./CONTRIBUTING.md)。
