//! Credential storage backed by OHOS Asset Store.

use napi_derive_ohos::napi;
use napi_ohos::{Error, Result};
use openharmony_ability::{
    impl_bridge_napi_type, AsyncBridge, BridgeCallOptions, BridgeContextRequirement, BridgePlugin,
    BridgeRuntime, OpenHarmonyApp,
};

pub struct CredentialsBridgePlugin;

impl BridgePlugin for CredentialsBridgePlugin {
    type Mode = AsyncBridge;
    const ID: &'static str = "ohos.credentials";
    const REQUIRED_CONTEXTS: &'static [BridgeContextRequirement] =
        &[BridgeContextRequirement::Ability];
}

#[napi(object)]
#[derive(Clone, Debug)]
pub struct CredentialKeyRequest {
    pub alias: String,
}
impl_bridge_napi_type!(CredentialKeyRequest, "ohos.credentials.KeyRequest");

#[napi(object)]
#[derive(Clone, Debug)]
pub struct CredentialWriteRequest {
    pub alias: String,
    pub username: String,
    pub password: Vec<u8>,
}
impl_bridge_napi_type!(CredentialWriteRequest, "ohos.credentials.WriteRequest");

#[napi(object)]
#[derive(Clone, Debug)]
pub struct CredentialReadResponse {
    pub username: Option<String>,
    pub password: Option<Vec<u8>>,
}
impl_bridge_napi_type!(CredentialReadResponse, "ohos.credentials.ReadResponse");

#[napi(object)]
#[derive(Clone, Debug)]
pub struct CredentialAcknowledgement {
    pub accepted: bool,
}
impl_bridge_napi_type!(
    CredentialAcknowledgement,
    "ohos.credentials.Acknowledgement"
);

#[derive(Clone)]
pub struct CredentialsClient {
    bridge: BridgeRuntime,
}

impl CredentialsClient {
    pub fn new(app: &OpenHarmonyApp) -> Result<Self> {
        Ok(Self {
            bridge: app.bridge()?,
        })
    }

    pub async fn write(&self, alias: String, username: String, password: Vec<u8>) -> Result<()> {
        validate_alias(&alias)?;
        let response = self
            .bridge
            .call_async::<CredentialsBridgePlugin, CredentialWriteRequest, CredentialAcknowledgement>(
                "write",
                CredentialWriteRequest { alias, username, password },
                BridgeCallOptions::default(),
            )
            .await?;
        if response.accepted {
            Ok(())
        } else {
            Err(Error::from_reason("Credential write rejected"))
        }
    }

    pub async fn read(&self, alias: String) -> Result<Option<(String, Vec<u8>)>> {
        validate_alias(&alias)?;
        let response = self
            .bridge
            .call_async::<CredentialsBridgePlugin, CredentialKeyRequest, CredentialReadResponse>(
                "read",
                CredentialKeyRequest { alias },
                BridgeCallOptions::default(),
            )
            .await?;
        match (response.username, response.password) {
            (Some(username), Some(password)) => Ok(Some((username, password))),
            (None, None) => Ok(None),
            _ => Err(Error::from_reason("Incomplete credential response")),
        }
    }

    pub async fn delete(&self, alias: String) -> Result<()> {
        validate_alias(&alias)?;
        let response = self
            .bridge
            .call_async::<CredentialsBridgePlugin, CredentialKeyRequest, CredentialAcknowledgement>(
                "delete",
                CredentialKeyRequest { alias },
                BridgeCallOptions::default(),
            )
            .await?;
        if response.accepted {
            Ok(())
        } else {
            Err(Error::from_reason("Credential deletion rejected"))
        }
    }
}

fn validate_alias(alias: &str) -> Result<()> {
    if alias.is_empty() || alias.as_bytes().len() > 256 {
        Err(Error::from_reason("Credential alias must be 1..=256 bytes"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openharmony_ability::BridgeNapiType;

    #[test]
    fn named_contracts_are_stable() {
        assert_eq!(
            CredentialKeyRequest::TYPE_NAME,
            "ohos.credentials.KeyRequest"
        );
        assert_eq!(
            CredentialWriteRequest::TYPE_NAME,
            "ohos.credentials.WriteRequest"
        );
        assert_eq!(
            CredentialReadResponse::TYPE_NAME,
            "ohos.credentials.ReadResponse"
        );
        assert_eq!(
            CredentialAcknowledgement::TYPE_NAME,
            "ohos.credentials.Acknowledgement"
        );
    }
}
