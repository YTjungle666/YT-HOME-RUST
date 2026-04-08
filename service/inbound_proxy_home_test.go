package service

import (
	"testing"

	"github.com/alireza0/s-ui/database/model"
)

func boolPtr(v bool) *bool {
	return &v
}

func TestHasInboundProxyHomeEnabled(t *testing.T) {
	inbounds := []*model.Inbound{
		{Tag: "mixed-default", Type: "mixed"},
		{Tag: "tun-enabled", Type: "tun", ProxyHome: boolPtr(true)},
	}
	if HasInboundProxyHomeEnabled(inbounds) {
		t.Fatal("expected false when no supported inbound enables proxy_home")
	}

	inbounds = append(inbounds, &model.Inbound{Tag: "http-enabled", Type: "http", ProxyHome: boolPtr(true)})
	if !HasInboundProxyHomeEnabled(inbounds) {
		t.Fatal("expected true when a supported inbound enables proxy_home")
	}
}
