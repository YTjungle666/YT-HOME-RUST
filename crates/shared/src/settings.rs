use std::collections::BTreeMap;

pub const APP_NAME: &str = "s-ui";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const SESSION_COOKIE: &str = "s-ui";

pub const DEFAULT_CONFIG_JSON: &str = r#"{
  "log": {
    "level": "info"
  },
  "dns": {
    "servers": [],
    "rules": []
  },
  "route": {
    "rules": [
      {
        "action": "sniff"
      },
      {
        "protocol": [
          "dns"
        ],
        "action": "hijack-dns"
      }
    ]
  },
  "experimental": {}
}"#;

pub fn default_settings() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("config", DEFAULT_CONFIG_JSON),
        ("sessionMaxAge", "0"),
        ("subCertFile", ""),
        ("subClashExt", ""),
        ("subDomain", ""),
        ("subEncode", "true"),
        ("subJsonExt", ""),
        ("subKeyFile", ""),
        ("subListen", ""),
        ("subPath", "/sub/"),
        ("subPort", "2096"),
        ("subShowInfo", "false"),
        ("subURI", ""),
        ("subUpdates", "12"),
        ("timeLocation", "Asia/Tehran"),
        ("trafficAge", "30"),
        ("version", APP_VERSION),
        ("webCertFile", ""),
        ("webDomain", ""),
        ("webKeyFile", ""),
        ("webListen", ""),
        ("webPath", "/"),
        ("webPort", "80"),
        ("webURI", ""),
    ])
}
