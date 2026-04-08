package util

import (
	"encoding/json"
	"testing"

	"github.com/alireza0/s-ui/database/model"
)

func TestAddTLSFallsBackToRealityHandshakeServerName(t *testing.T) {
	tls := &model.Tls{
		Server: json.RawMessage(`{
			"enabled": true,
			"server_name": "",
			"reality": {
				"enabled": true,
				"handshake": {
					"server": "edge.speedtest.ytjungle.top",
					"server_port": 443
				},
				"short_id": ["0123456789abcdef"]
			}
		}`),
		Client: json.RawMessage(`{
			"reality": {
				"public_key": "test-public-key"
			},
			"utls": {
				"enabled": true,
				"fingerprint": "chrome"
			}
		}`),
	}

	out := map[string]interface{}{}
	addTls(&out, tls)

	tlsConfig := out["tls"].(map[string]interface{})
	if got := tlsConfig["server_name"]; got != "edge.speedtest.ytjungle.top" {
		t.Fatalf("expected fallback server_name, got %v", got)
	}
}

func TestPrepareTLSFallsBackToRealityHandshakeServerName(t *testing.T) {
	tls := &model.Tls{
		Server: json.RawMessage(`{
			"enabled": true,
			"server_name": "",
			"reality": {
				"enabled": true,
				"handshake": {
					"server": "edge.speedtest.ytjungle.top",
					"server_port": 443
				},
				"short_id": ["0123456789abcdef"]
			}
		}`),
		Client: json.RawMessage(`{
			"reality": {
				"public_key": "test-public-key"
			},
			"utls": {
				"enabled": true,
				"fingerprint": "chrome"
			}
		}`),
	}

	prepared := prepareTls(tls)
	if got := prepared["server_name"]; got != "edge.speedtest.ytjungle.top" {
		t.Fatalf("expected fallback server_name, got %v", got)
	}
}
