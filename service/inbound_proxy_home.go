package service

import "github.com/alireza0/s-ui/database/model"

func supportsInboundProxyHome(inboundType string) bool {
	switch inboundType {
	case "mixed", "socks", "http", "shadowsocks", "vmess", "trojan", "naive", "hysteria", "shadowtls", "tuic", "hysteria2", "vless", "anytls":
		return true
	}
	return false
}

func HasInboundProxyHomeEnabled(inbounds []*model.Inbound) bool {
	for _, inbound := range inbounds {
		if inbound != nil && supportsInboundProxyHome(inbound.Type) && inbound.IsProxyHomeEnabled() {
			return true
		}
	}
	return false
}
