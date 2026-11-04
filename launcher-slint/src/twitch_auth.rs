use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    #[serde(default)]
    pub token_type: String,
}

pub async fn refresh_twitch_token(
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<TokenResponse, String> {
    if client_id.is_empty() || client_secret.is_empty() || refresh_token.is_empty() {
        return Err("Не заполнены Twitch Client ID, Client Secret или Refresh Token".to_string());
    }

    let client = reqwest::Client::new();
    let mut params = HashMap::new();
    params.insert("grant_type", "refresh_token");
    params.insert("refresh_token", refresh_token);
    params.insert("client_id", client_id);
    params.insert("client_secret", client_secret);

    let res = client
        .post("https://id.twitch.tv/oauth2/token")
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Сетевая ошибка при обращении к Twitch OAuth: {}", e))?;

    let status = res.status();
    if !status.is_success() {
        let err_body = res.text().await.unwrap_or_default();
        return Err(format!("Twitch API вернул HTTP {}: {}", status.as_u16(), err_body));
    }

    res.json::<TokenResponse>()
        .await
        .map_err(|e| format!("Ошибка парсинга ответа Twitch OAuth: {}", e))
}
