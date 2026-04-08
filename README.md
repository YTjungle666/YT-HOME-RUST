# YT HOME

**A self-hosted home access control plane built on sing-box**

![Release](https://img.shields.io/github/v/release/YTjungle666/YT-HOME?display_name=tag)
![CI](https://img.shields.io/github/actions/workflow/status/YTjungle666/YT-HOME/ci.yml?branch=main&label=ci)
![License](https://img.shields.io/badge/license-GPL%20v3-blue.svg)

YT HOME is a private deployment panel for people who want one thing done well: publish a secure public entry to their home network, distribute client subscriptions cleanly, and keep internal services reachable without turning the project into a pile of routing fragments and manual configs.

It is designed for home labs and private infrastructure: `PVE`, `NAS`, `router`, `IPMI`, `Home Assistant`, internal dashboards, file services, and any LAN-only web service you want to reach from the outside through a controlled proxy path.

## Why YT HOME

- **Home-first design**: optimized for public access back into your own LAN, not just generic node management.
- **Single-node proxy-home mode**: enable return-home access only on the node you choose, without breaking your normal multi-node subscriptions.
- **Reality-ready delivery**: works with modern `VLESS + Reality` style deployments and clean client-side imports.
- **Operational visibility**: panel, clients, inbounds, subscriptions and runtime status stay in one place.
- **Self-hosted release model**: this repository ships binary releases and CI validation, without publishing Docker images by default.

## Product Highlights

- Multi-protocol inbound management based on `sing-box`
- Client and subscription management
- JSON / Clash subscription output
- Dedicated “proxy home” node behavior for return-home access
- Reality-compatible TLS configuration
- Runtime logs, traffic, client stats and system status
- Linux and Windows release packaging through GitHub Actions

## Typical Workflow

1. Deploy YT HOME inside your home network.
2. Expose the required inbound ports through your router.
3. Publish a `VLESS + Reality` node with your public domain.
4. Turn on **Proxy Home** only for the inbound that should act as your return-home entry.
5. Import that node’s single-inbound subscription into your client.
6. Reach private services through the server-side LAN path.

## Default Access

- Panel port: `80`
- Panel path: `/`
- Subscription port: `2096`
- Subscription path: `/sub/`
- Default admin: `admin`

Example:

- Panel: `http://your-host/`
- Subscription base: `http://your-host:2096/sub/`

## Install

### Linux / macOS

```bash
bash <(curl -Ls https://raw.githubusercontent.com/YTjungle666/YT-HOME/main/install.sh)
```

### Manual Release Install

1. Open [GitHub Releases](https://github.com/YTjungle666/YT-HOME/releases/latest)
2. Download the archive matching your architecture
3. Extract it to `/usr/local/s-ui`
4. Enable the service:

```bash
systemctl daemon-reload
systemctl enable s-ui --now
```

### Local Docker Compose Build

This repository keeps Docker support for self-build use, but does **not** publish Docker images as part of release delivery.

```bash
git clone https://github.com/YTjungle666/YT-HOME
cd YT-HOME
docker compose up -d --build
```

## Release and CI

- `ci.yml` builds the frontend, runs Go tests, and verifies the backend can compile.
- `release.yml` packages Linux release archives.
- `windows.yml` packages Windows release archives.
- No Docker image publishing workflow is included.

## Repository Layout

- `frontend/`: Vue 3 + Vuetify frontend source
- `service/`: application service layer
- `sub/`: subscription generation logic
- `web/`: embedded frontend assets for backend serving
- `.github/workflows/`: CI and release automation

## Development

### Build Frontend

```bash
cd frontend
npm ci
npm run build
cd ..
mkdir -p web/html
rm -rf web/html/*
cp -R frontend/dist/. web/html/
```

### Build Backend

```bash
go test ./service/... ./sub/... ./util/...
go build ./...
```

### Run Locally

```bash
SUI_DB_FOLDER=db SUI_DEBUG=true ./sui
```

## Notes

- Deploy responsibly and only on infrastructure you control.
- If you operate behind residential NAT, make sure your router forwarding and public domain resolution are already correct before troubleshooting protocol behavior.
- If you use `Reality`, prefer a domain you control as the handshake target instead of depending on unrelated third-party sites.
