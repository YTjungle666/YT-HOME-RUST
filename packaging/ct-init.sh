#!/bin/sh

set -eu

mkdir -p /app/db
cd /app

export SUI_WEB_DIR="/app/web"
export SUI_MIGRATIONS_DIR="/app/migrations"

exec /app/sui
