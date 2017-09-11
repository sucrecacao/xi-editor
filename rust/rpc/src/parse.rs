// Copyright 2017 Google Inc. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Parsing of raw JSON messages into RPC objects.

use serde_json::{self, Value, Error as JsonError};
use serde::de::DeserializeOwned;

use error::RemoteError;


/// A unique identifier attached to request RPCs.
type RequestId = u64;

/// An RPC response, received from the peer.
pub type Response = Result<Value, RemoteError>;

/// An internal type used during initial JSON parsing.
///
/// Wraps an arbitrary JSON object, which may be any valid or invalid
/// RPC message. This allows initial parsing and response handling to
/// occur on the read thread. If the message looks like a request, it
/// is passed to the main thread for handling.
pub struct RpcObject(Value);

#[derive(Debug, Clone, PartialEq)]
/// An RPC call, which may be either a notification or a request.
pub enum Call<N, R> {
    /// An id and an RPC Request
    Request(RequestId, R),
    /// An RPC Notification
    Notification(N),
    /// A malformed request: the request contained an id, but could
    /// not be parsed. The client will receive an error.
    InvalidRequest(RequestId, RemoteError),
}

impl RpcObject {
    /// Returns the 'id' of the underlying object, if present.
    pub fn get_id(&self) -> Option<RequestId> {
        self.0.get("id").and_then(Value::as_u64)
    }

    /// Returns `true` if this object looks like an RPC response;
    /// that is, if it has an 'id' field and does _not_ have a 'method'
    /// field.
    pub fn is_response(&self) -> bool {
        self.0.get("id").is_some() && self.0.get("method").is_none()
    }

    /// Attempts to convert the underlying `Value` into an RPC response
    /// object, and returns the result.
    ///
    /// The caller is expected to verify that the object is a response
    /// before calling this method.
    ///
    /// # Errors
    ///
    /// If the `Value` is not a well formed response object, this will
    /// return a `String` containing an error message. The caller should
    /// print this message and exit.
    pub fn into_response(mut self) -> Result<Response, String> {
        let _ = self.get_id()
            .ok_or("Response requires 'id' field.".to_string())?;

        if self.0.get("result").is_some() == self.0.get("error").is_some() {
            return Err("RPC response must contain exactly one of\
                       'error' or 'result' fields.".into());
        }
        let result = self.0.as_object_mut()
            .and_then(|obj| obj.remove("result"));
        //let id = id.map(Value::from).unwrap_or(Value::Null);
        match result {
            Some(r) => Ok(Ok(r)),
            None => {
                let error = self.0.as_object_mut()
                    .and_then(|obj| obj.remove("error")).unwrap();
                match serde_json::from_value::<RemoteError>(error) {
                    Ok(e) => Ok(Err(e)),
                    Err(e) => Err(format!("Error handling response: {:?}", e)),
                }
            }
        }
    }

    /// Attempts to convert the underlying `Value` into either an RPC
    /// notification or request.
    ///
    /// # Errors
    ///
    /// Returns a `serde_json::Error` if the `Value` cannot be converted
    /// to one of the expected types.
    pub fn into_rpc<N, R>(self) -> Result<Call<N, R>, JsonError>
    where N: DeserializeOwned,
          R: DeserializeOwned,
    {
        let id = self.get_id();
        match id {
            Some(id) => {
                match serde_json::from_value::<R>(self.0) {
                    Ok(resp) => Ok(Call::Request(id, resp)),
                    Err(err) => Ok(Call::InvalidRequest(id, err.into())),
                }
            }
            None => {
                let result = serde_json::from_value::<N>(self.0)?;
                Ok(Call::Notification(result))
            }
        }
    }
}

impl From<Value> for RpcObject {
    fn from(v: Value) -> RpcObject {
        RpcObject(v)
    }
}


#[cfg(test)]
mod tests {

    use serde_json;
    use super::*;

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    #[serde(rename_all = "snake_case")]
    #[serde(tag = "method", content = "params")]
    enum TestR {
        NewView { file_path: Option<String> },
        OldView { file_path: usize },
    }

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    #[serde(rename_all = "snake_case")]
    #[serde(tag = "method", content = "params")]
    enum TestN {
        CloseView { view_id: String },
        Save { view_id: String, file_path: String },
    }

    #[test]
    fn request_success() {
        let json = r#"{"id":0,"method":"new_view","params":{}}"#;
        let p: RpcObject = serde_json::from_str::<Value>(json).unwrap().into();
        assert!(!p.is_response());
        let req = p.into_rpc::<TestN, TestR>().unwrap();
        assert_eq!(req, Call::Request(0, TestR::NewView { file_path: None }));
    }

    #[test]
    fn request_failure() {
        // method does not exist
        let json = r#"{"id":0,"method":"new_truth","params":{}}"#;
        let p: RpcObject = serde_json::from_str::<Value>(json).unwrap().into();
        let req = p.into_rpc::<TestN, TestR>().unwrap();
        let is_ok = match req {
            Call::InvalidRequest(0, _) => true,
            _ => false,
        };
        if !is_ok {
            panic!("{:?}", req);
        }
    }

    #[test]
    fn notif_with_id() {
        // method is a notification, should not have ID
        let json = r#"{"id":0,"method":"close_view","params":{"view_id": "view-id-1"}}"#;
        let p: RpcObject = serde_json::from_str::<Value>(json).unwrap().into();
        let req = p.into_rpc::<TestN, TestR>().unwrap();
        let is_ok = match req {
            Call::InvalidRequest(0, _) => true,
            _ => false,
        };
        if !is_ok {
            panic!("{:?}", req);
        }
    }

    #[test]
    fn test_resp_err() {
        let json = r#"{"id":5,"error":{"code":420, "message":"chill out"}}"#;
        let p: RpcObject = serde_json::from_str::<Value>(json).unwrap().into();
        assert!(p.is_response());
        let resp  = p.into_response().unwrap();
        assert_eq!(resp, Err(RemoteError::custom(420, "chill out", None)));
    }

    #[test]
    fn test_resp_result() {
        let json = r#"{"id":5,"result":"success!"}"#;
        let p: RpcObject = serde_json::from_str::<Value>(json).unwrap().into();
        assert!(p.is_response());
        let resp  = p.into_response().unwrap();
        assert_eq!(resp, Ok(json!("success!")));
    }

    #[test]
    fn test_err() {
        let json = r#"{"code": -32600, "message": "Invalid Request"}"#;
        let e = serde_json::from_str::<RemoteError>(json).unwrap();
        assert_eq!(e, RemoteError::InvalidRequest(None));
    }
}
