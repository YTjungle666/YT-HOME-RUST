#!/bin/sh

set -eu

cd frontend
npm ci
npm run lint -- --max-warnings=0
npm run typecheck
npm run build

cd ..
echo "Backend"

mkdir -p web
rm -fr web/*
cp -R frontend/dist/. web/
mkdir -p migrations
rm -fr migrations/*
cp -R crates/infra-db/migrations/. migrations/

cargo build --release -p app
sh ./scripts/fetch-sing-box.sh linux amd64 . "${SING_BOX_VERSION:-1.13.5}"
cp target/release/app ./sui
chmod +x ./sui ./sing-box
