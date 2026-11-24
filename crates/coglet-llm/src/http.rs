//! Non-streaming, text-only provider adapters. All configuration is trusted host policy.
//! Invoke on a bounded blocking outbox worker, never an async reactor or guest handle.
use crate::{Call, Completion, Provider, Result, failure};
use reqwest::{
    Url,
    blocking::Client,
    header::{AUTHORIZATION, HeaderValue},
};
use serde_json::{Value, json};
use std::{io::Read, time::Duration};

#[derive(Clone, Copy)]
pub enum Api {
    OpenAi,
    Anthropic,
}
/// Pricing is integer micro-units per token, rounded UP by the deployment if needed.
/// Use only text models with known byte-token bounds and no ancillary charges.
pub struct HttpConfig {
    pub api: Api,
    /// Full trusted URL: https in production, http only for literal loopback fakes/proxy.
    pub endpoint: String,
    pub model: String,
    pub secret_name: String,
    pub max_output_tokens: u32,
    pub max_prompt_bytes: u32,
    pub input_micro_per_token: i64,
    pub output_micro_per_token: i64,
    pub timeout: Duration,
}
/// Implement with the hub secret store. Secrets are resolved per call, not journaled.
/// Errors must not include credentials; this adapter deliberately discards their text.
pub trait SecretStore {
    fn get(&mut self, name: &str) -> Result<String>;
}
impl<F: FnMut(&str) -> Result<String>> SecretStore for F {
    fn get(&mut self, name: &str) -> Result<String> {
        self(name)
    }
}
pub struct HttpProvider<S> {
    config: HttpConfig,
    endpoint: Url,
    client: Client,
    secrets: S,
    ceiling: i64,
}
impl<S: SecretStore> HttpProvider<S> {
    pub fn new(config: HttpConfig, secrets: S) -> Result<Self> {
        let endpoint = Url::parse(&config.endpoint).map_err(|_| "invalid provider endpoint")?;
        let loopback = endpoint
            .host_str()
            .is_some_and(|h| h == "127.0.0.1" || h == "[::1]" || h == "::1");
        if !(endpoint.scheme() == "https" || endpoint.scheme() == "http" && loopback)
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err("unsafe provider endpoint".into());
        }
        if config.max_output_tokens == 0
            || config.max_prompt_bytes == 0
            || config.model.is_empty()
            || config.secret_name.is_empty()
            || config.input_micro_per_token < 0
            || config.output_micro_per_token < 0
            || config.timeout.is_zero()
        {
            return Err("invalid provider policy".into());
        }
        // At most one token per UTF-8 byte plus a conservative 1024-token fixed wrapper.
        // Models not satisfying this configured deployment contract must not be routed here.
        let ceiling = (i64::from(config.max_prompt_bytes) + 1024)
            .checked_mul(config.input_micro_per_token)
            .and_then(|x| {
                i64::from(config.max_output_tokens)
                    .checked_mul(config.output_micro_per_token)
                    .and_then(|y| x.checked_add(y))
            })
            .ok_or("pricing overflow")?;
        let client = Client::builder()
            .timeout(config.timeout)
            .connect_timeout(config.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .build()
            .map_err(|_| "provider client setup failed")?;
        Ok(Self {
            config,
            endpoint,
            client,
            secrets,
            ceiling,
        })
    }
    pub fn required_ceiling(&self) -> i64 {
        self.ceiling
    }
}
impl<S: SecretStore> Provider for HttpProvider<S> {
    fn complete(&mut self, call: &Call) -> Result<Completion> {
        let id = call.request["id"]
            .as_str()
            .ok_or("missing request identity")?;
        let rejected = |code, msg| Ok(failure(id, code, msg, 0));
        let input = call.request["input"]
            .as_object()
            .ok_or("invalid provider input")?;
        let Some(prompt) = input.get("prompt").and_then(Value::as_str) else {
            return rejected("invalid_argument", "text prompt required");
        };
        if input.len() != 1 || prompt.len() > self.config.max_prompt_bytes as usize {
            return rejected(
                "invalid_argument",
                "unsupported or oversized provider input",
            );
        }
        if call.ceiling < self.ceiling {
            return rejected(
                "resource_exhausted",
                "route ceiling below configured provider maximum",
            );
        }
        let secret = match self.secrets.get(&self.config.secret_name) {
            Ok(s) => s,
            Err(_) => return rejected("unavailable", "provider secret unavailable"),
        };
        let mut secret = match HeaderValue::from_str(&secret) {
            Ok(s) if !s.is_empty() => s,
            _ => return rejected("unavailable", "provider secret invalid"),
        };
        secret.set_sensitive(true);
        let mut body = json!({"model":self.config.model,"messages":[{"role":"user","content":prompt}],"stream":false});
        let mut builder = self.client.post(self.endpoint.clone());
        match self.config.api {
            Api::OpenAi => {
                body["max_completion_tokens"] = json!(self.config.max_output_tokens);
                let mut bearer = HeaderValue::from_str(&format!(
                    "Bearer {}",
                    secret.to_str().map_err(|_| "invalid provider secret")?
                ))
                .map_err(|_| "invalid provider secret")?;
                bearer.set_sensitive(true);
                builder = builder.header(AUTHORIZATION, bearer);
            }
            Api::Anthropic => {
                body["max_tokens"] = json!(self.config.max_output_tokens);
                builder = builder
                    .header("x-api-key", secret)
                    .header("anthropic-version", "2023-06-01");
            }
        }
        let response = builder
            .json(&body)
            .send()
            .map_err(|_| "uncertain provider transport")?;
        let status = response.status().as_u16();
        let code = match status {
            400 | 413 | 422 => Some("invalid_argument"),
            401 | 403 => Some("permission_denied"),
            404 => Some("not_found"),
            429 => Some("resource_exhausted"),
            _ => None,
        };
        if let Some(code) = code {
            return rejected(code, "provider rejected request");
        }
        if !(200..300).contains(&status) {
            return Err("uncertain provider response".into());
        }
        let mut raw = Vec::new();
        response
            .take(1_048_577)
            .read_to_end(&mut raw)
            .map_err(|_| "uncertain provider body")?;
        if raw.len() > 1_048_576 {
            return Err("oversized provider receipt".into());
        }
        let value: Value = serde_json::from_slice(&raw).map_err(|_| "invalid provider receipt")?;
        let (text, input, output) = match self.config.api {
            Api::OpenAi => {
                let choices = value["choices"]
                    .as_array()
                    .ok_or("missing provider choices")?;
                if choices.len() != 1 || !choices[0]["message"]["tool_calls"].is_null() {
                    return Err("unsupported provider choices".into());
                }
                (
                    choices[0]["message"]["content"]
                        .as_str()
                        .ok_or("missing provider text")?
                        .to_owned(),
                    value["usage"]["prompt_tokens"].as_u64(),
                    value["usage"]["completion_tokens"].as_u64(),
                )
            }
            Api::Anthropic => {
                for key in ["cache_creation_input_tokens", "cache_read_input_tokens"] {
                    if value["usage"]
                        .get(key)
                        .is_some_and(|n| n.as_u64() != Some(0))
                    {
                        return Err("unsupported cache billing".into());
                    }
                }
                let blocks = value["content"]
                    .as_array()
                    .ok_or("missing provider content")?;
                let mut text = String::new();
                for block in blocks {
                    if block["type"] != "text" {
                        return Err("unsupported provider content".into());
                    }
                    text.push_str(block["text"].as_str().ok_or("missing provider text")?);
                }
                (
                    text,
                    value["usage"]["input_tokens"].as_u64(),
                    value["usage"]["output_tokens"].as_u64(),
                )
            }
        };
        let input = input.ok_or("missing provider usage")?;
        let output = output.ok_or("missing provider usage")?;
        if input > u64::from(self.config.max_prompt_bytes) + 1024
            || output > u64::from(self.config.max_output_tokens)
        {
            return Err("provider exceeded token contract".into());
        }
        let cost = i64::try_from(input)
            .ok()
            .and_then(|n| n.checked_mul(self.config.input_micro_per_token))
            .and_then(|x| {
                i64::try_from(output)
                    .ok()
                    .and_then(|n| n.checked_mul(self.config.output_micro_per_token))
                    .and_then(|y| x.checked_add(y))
            })
            .ok_or("provider cost overflow")?;
        Ok(Completion {
            reply: json!({"request_id":id,"status":"succeeded","output":{"text":text,"input_tokens":input,"output_tokens":output}}),
            cost,
        })
    }
}
