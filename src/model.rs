/// This module describes the main logic of web-settings service
use super::config::ConfigItem;
use futures::future;
use futures::future::BoxFuture;
use futures::prelude::*;
use futures_util::future::FutureExt;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
// FIXME: this is not good dependency practice
use gotham_derive::StateData;

#[derive(Clone, PartialEq, Eq, Hash, Deserialize, Serialize, StateData)]
pub struct Secret(String);

impl ToString for Secret {
    fn to_string(&self) -> String {
        self.0.clone()
    }
}

impl From<Secret> for String {
    fn from(item: Secret) -> Self {
        item.0
    }
}

enum ClientSt {
    Created,
    Submitted,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Values {
    pub revision: u32,
    pub values: Vec<ConfigItem>,
}

type Message = Result<Values, ()>;

use futures::channel::oneshot;
use futures::channel::oneshot::{Receiver, Sender};

struct Client {
    settings: Vec<ConfigItem>,
    st: ClientSt,
    sender: Option<Sender<Message>>,
    revision: u32,
}

impl Client {
    fn new(settings: Vec<ConfigItem>) -> Self {
        Self {
            settings,
            st: ClientSt::Created,
            sender: None,
            revision: 0,
        }
    }

    /// Notify receiver about changed settings
    fn send(&mut self) {
        match self.sender.take() {
            Some(s) => s.send(Ok(self.current_values())).map_err(|_| ()).unwrap(),
            None => eprintln!("no sender."),
        }
    }

    fn get_reciver(&mut self) -> Receiver<Message> {
        self.send_err();
        let (sender, receiver) = oneshot::channel::<Message>();
        self.sender = Some(sender);
        receiver
    }

    fn send_err(&mut self) {
        match self.sender.take() {
            Some(s) => s.send(Err(())).map_err(|_| ()).unwrap(),
            None => eprintln!("no sender."),
        }
    }

    fn current_values(&self) -> Values {
        Values {
            revision: self.revision,
            values: self.settings.clone(),
        }
    }
}

use std::collections::HashMap;

struct Payload<T> {
    data: T,
    timestamp: u64,
}

struct KeyStorage<T> {
    expiration: u32,
    keys: HashMap<String, Payload<T>>,
    rng: SecretRng,
}

impl<T> KeyStorage<T> {
    pub fn new(expiration: u32) -> Self {
        Self {
            expiration,
            keys: HashMap::new(),
            rng: make_rng(),
        }
    }

    fn timestamp() -> u64 {
        use std::time::UNIX_EPOCH;
        // Must not panic because now is later than epoch
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    pub fn new_key(&mut self, data: T) -> String {
        use std::collections::hash_map::Entry;
        for _ in 0..2 {
            let key = self.random_key();
            match self.keys.entry(key.clone()) {
                Entry::Vacant(v) => {
                    v.insert(Payload {
                        data: data,
                        timestamp: Self::timestamp(),
                    });
                    return key;
                }
                Entry::Occupied(_) => {
                    self.cleanup();
                    continue;
                }
            }
        }
        panic!("Failed to generate unique key");
    }

    pub fn take_data(&mut self, key: &str) -> Result<T, &'static str> {
        match self.keys.remove(key) {
            Some(v) => {
                let t = Self::timestamp();
                if t - v.timestamp < self.expiration as u64 {
                    Ok(v.data)
                } else {
                    Err("Key expired")
                }
            }
            None => Err("Invalid key"),
        }
    }

    fn cleanup(&mut self) {}

    fn random_key(&mut self) -> String {
        // FIXME: This is short, is it a security fault?
        let mut bytes = [0u8; 4];
        self.rng.fill_bytes(&mut bytes);
        base64::encode_config(&bytes[..], base64::URL_SAFE_NO_PAD)
    }
}

pub struct Model {
    clients: HashMap<Secret, Client>,
    keys: KeyStorage<Secret>,
    rng: SecretRng,
}

impl Model {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            keys: KeyStorage::new(10 * 60),
            rng: make_rng(),
        }
    }

    /// Creates new clinet with given settings
    /// and returs single time access key
    pub fn new_client(&mut self, settings: Vec<ConfigItem>) -> (String, Secret) {
        use std::collections::hash_map::Entry;
        for _ in 0..10 {
            let secret = self.random_secret();
            match self.clients.entry(secret.clone()) {
                Entry::Vacant(v) => {
                    v.insert(Client::new(settings));
                    return (self.keys.new_key(secret.clone()), secret);
                }
                Entry::Occupied(_) => {
                    self.cleanup();
                    continue;
                }
            }
        }
        panic!("Failed to create unique secret")
    }

    pub fn remove_client(&mut self, sid: &str) -> Result<(), &'static str> {
        // FIXME: create &Secret from &str
        self.clients
            .remove(&Secret(sid.to_owned()))
            .map(|_| ())
            .ok_or("session does not exists")
    }

    /// Returns a Future that waits for values to be updated
    pub fn values(&mut self, sid: &str, revision: u32) -> BoxFuture<'static, Message> {
        let client = self.clients.get_mut(&Secret(sid.to_owned())).ok_or(());
        let client = match client {
            Ok(c) => c,
            Err(_) => return future::err(()).boxed(),
        };

        if revision < client.revision {
            // we have newer revision immediately
            return future::ok(client.current_values()).boxed();
        } else if revision == client.revision {
            // recreate communication channel
            let f = client.get_reciver().map(|res| res.unwrap_or(Err(())));
            return Box::pin(f);
        } else {
            // must never happen
            return future::err(()).boxed();
        }
    }

    pub fn auth(&mut self, key: &str) -> Result<Secret, &'static str> {
        self.keys.take_data(key)
    }

    pub fn settings(&mut self, s: &Secret) -> Result<&Vec<ConfigItem>, &'static str> {
        self.clients
            .get(s)
            .map(|c| &c.settings)
            .ok_or("invalid session")
    }

    pub fn update_settings(
        &mut self,
        s: &Secret,
        values: HashMap<String, String>,
    ) -> Result<(), &'static str> {
        let client = self.clients.get_mut(s).ok_or("invalid session")?;

        for s in client.settings.iter_mut() {
            match values.get(&s.name) {
                Some(v) => {
                    if !s.value.try_set_value(v) {
                        return Err("bad value");
                    }
                }
                None => {}
            }
        }
        client.revision += 1;
        client.send();
        Ok(())
    }

    fn random_secret(&mut self) -> Secret {
        let mut bytes = [0u8; 64];
        self.rng.fill_bytes(&mut bytes);
        Secret(base64::encode_config(&bytes[..], base64::URL_SAFE_NO_PAD))
    }

    fn cleanup(&mut self) {
        unimplemented!()
    }
}

use rand::rngs::adapter::ReseedingRng;
use rand_chacha::rand_core::OsRng;
use rand_chacha::rand_core::RngCore;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaChaCore;

/// This generator is used for cookies in gotham
/// I assume it is ok for our purpose as well
type SecretRng = ReseedingRng<ChaChaCore, OsRng>;

fn make_rng() -> SecretRng {
    let rng = ChaChaCore::from_entropy();
    // Reseed every 32KiB.
    ReseedingRng::new(rng, 32_768, OsRng)
}
