//   Copyright 2023. The Tari Project
//
//   Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
//   following conditions are met:
//
//   1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
//   disclaimer.
//
//   2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
//   following disclaimer in the documentation and/or other materials provided with the distribution.
//
//   3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
//   products derived from this software without specific prior written permission.
//
//   THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
//   INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
//   DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
//   SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
//   SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
//   WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
//   USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use std::{
    convert::TryFrom,
    ffi::{c_void, CString},
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex, Once},
};

use async_trait::async_trait;
use libc::{c_char, c_int, c_uchar, c_uint};
use minotari_app_utilities::identity_management::setup_node_identity;
use tari_chat_client::{database, error::Error as ClientError, ChatClient as ChatClientTrait};
use tari_common::configuration::Network;
use tari_common_types::tari_address::TariAddress;
use tari_comms::{
    multiaddr::Multiaddr,
    peer_manager::{Peer, PeerFeatures},
};
use tari_contacts::contacts_service::{
    service::ContactOnlineStatus,
    types::{Message, MessageId},
};

use crate::{chat_client::test_config, get_port};

extern "C" fn callback_contact_status_change(_state: *mut c_void) {
    let callback = ChatCallback::instance();
    *callback.contact_status_change.lock().unwrap() += 1;
}

extern "C" fn callback_message_received(_state: *mut c_void) {
    let callback = ChatCallback::instance();
    *callback.message_received.lock().unwrap() += 1;
}

extern "C" fn callback_delivery_confirmation_received(_state: *mut c_void) {
    let callback = ChatCallback::instance();
    *callback.delivery_confirmation_received.lock().unwrap() += 1;
}

extern "C" fn callback_read_confirmation_received(_state: *mut c_void) {
    let callback = ChatCallback::instance();
    *callback.read_confirmation_received.lock().unwrap() += 1;
}

type FfiChatClient = c_void;
type FfiTariAddress = c_void;
type FFiApplicationConfig = c_void;
type FFiContactsServiceHandle = c_void;
type FfiMessage = c_void;
type FfiChatByteVector = c_void;
type FfiMessageVector = c_void;
type FfiConversationalistsVector = c_void;

pub(crate) type CallbackContactStatusChange = unsafe extern "C" fn(*mut c_void);
pub(crate) type CallbackMessageReceived = unsafe extern "C" fn(*mut c_void);
pub(crate) type CallbackDeliveryConfirmationReceived = unsafe extern "C" fn(*mut c_void);
pub(crate) type CallbackReadConfirmationReceived = unsafe extern "C" fn(*mut c_void);

#[cfg_attr(windows, link(name = "minotari_chat_ffi.dll"))]
#[cfg_attr(not(windows), link(name = "minotari_chat_ffi"))]
extern "C" {
    pub fn create_chat_client(
        config: *mut FFiApplicationConfig,
        callback_contact_status_change: CallbackContactStatusChange,
        callback_message_received: CallbackMessageReceived,
        callback_delivery_confirmation_received: CallbackDeliveryConfirmationReceived,
        callback_read_confirmation_received: CallbackReadConfirmationReceived,
        tari_address: *mut FfiTariAddress,
        error_out: *const c_int,
    ) -> *mut FfiChatClient;
    pub fn sideload_chat_client(
        config: *mut FFiApplicationConfig,
        contact_handle: *mut FFiContactsServiceHandle,
        callback_contact_status_change: CallbackContactStatusChange,
        callback_message_received: CallbackMessageReceived,
        callback_delivery_confirmation_received: CallbackDeliveryConfirmationReceived,
        callback_read_confirmation_received: CallbackReadConfirmationReceived,
        tari_address: *mut FfiTariAddress,
        error_out: *const c_int,
    ) -> *mut FfiChatClient;
    pub fn create_chat_message(
        receiver: *mut FfiTariAddress,
        sender: *mut FfiTariAddress,
        message: *const c_char,
        error_out: *const c_int,
    ) -> *mut FfiMessage;
    pub fn send_chat_message(client: *mut FfiChatClient, message: *mut FfiMessage, error_out: *const c_int);
    pub fn add_chat_message_metadata(
        message: *mut FfiMessage,
        key: *mut FfiChatByteVector,
        data: *mut FfiChatByteVector,
        error_out: *const c_int,
    );
    pub fn add_chat_contact(client: *mut FfiChatClient, address: *mut FfiTariAddress, error_out: *const c_int);
    pub fn check_online_status(
        client: *mut FfiChatClient,
        address: *mut FfiTariAddress,
        error_out: *const c_int,
    ) -> c_uchar;
    pub fn get_chat_messages(
        client: *mut FfiChatClient,
        sender: *mut FfiTariAddress,
        limit: c_uint,
        page: c_uint,
        error_out: *const c_int,
    ) -> *mut FfiMessageVector;
    pub fn get_chat_message(
        client: *mut FfiChatClient,
        message_id: *mut FfiChatByteVector,
        error_out: *const c_int,
    ) -> *mut FfiMessage;
    pub fn destroy_chat_client(client: *mut FfiChatClient);
    pub fn chat_byte_vector_create(
        byte_array: *const c_uchar,
        element_count: c_uint,
        error_our: *const c_int,
    ) -> *mut FfiChatByteVector;
    pub fn send_read_confirmation_for_message(
        client: *mut FfiChatClient,
        message: *mut FfiMessage,
        error_out: *const c_int,
    );
    pub fn get_conversationalists(
        client: *mut FfiChatClient,
        error_out: *const c_int,
    ) -> *mut FfiConversationalistsVector;
}

#[derive(Debug)]
pub struct PtrWrapper(*mut FfiChatClient);
unsafe impl Send for PtrWrapper {}

#[derive(Debug)]
pub struct ChatFFI {
    ptr: Arc<Mutex<PtrWrapper>>,
    pub address: TariAddress,
}

struct Conversationalists(Vec<TariAddress>);
struct MessagesVector(Vec<Message>);

#[async_trait]
impl ChatClientTrait for ChatFFI {
    async fn add_contact(&self, address: &TariAddress) -> Result<(), ClientError> {
        let client = self.ptr.lock().unwrap();

        let address_ptr = Box::into_raw(Box::new(address.to_owned())) as *mut c_void;

        let error_out = Box::into_raw(Box::new(0));

        let result;
        unsafe {
            result = add_chat_contact(client.0, address_ptr, error_out);
            if *error_out != 0 {
                panic!("Error adding contact");
            }
        }

        Ok(result)
    }

    fn add_metadata(&self, message: Message, key: String, data: String) -> Result<Message, ClientError> {
        let message_ptr = Box::into_raw(Box::new(message)) as *mut c_void;
        let error_out = Box::into_raw(Box::new(0));

        let key_bytes = key.into_bytes();
        let len = i32::try_from(key_bytes.len()).expect("Truncation occurred") as c_uint;
        let byte_key = unsafe {
            let byte_key = chat_byte_vector_create(key_bytes.as_ptr(), len, error_out);
            if *error_out != 0 {
                panic!("Error creating byte vector");
            }
            byte_key
        };

        let data_bytes = data.into_bytes();
        let len = i32::try_from(data_bytes.len()).expect("Truncation occurred") as c_uint;
        let byte_data = unsafe {
            let byte_data = chat_byte_vector_create(data_bytes.as_ptr(), len, error_out);
            if *error_out != 0 {
                panic!("Error creating byte vector");
            }
            byte_data
        };

        unsafe {
            add_chat_message_metadata(message_ptr, byte_key, byte_data, error_out);
            if *error_out != 0 {
                panic!("Error adding metadata to message");
            }
            Ok(*Box::from_raw(message_ptr as *mut Message))
        }
    }

    async fn check_online_status(&self, address: &TariAddress) -> Result<ContactOnlineStatus, ClientError> {
        let client = self.ptr.lock().unwrap();

        let address_ptr = Box::into_raw(Box::new(address.clone())) as *mut FfiTariAddress;
        let error_out = Box::into_raw(Box::new(0));

        unsafe {
            let result = check_online_status(client.0, address_ptr, error_out);
            if *error_out != 0 {
                panic!("Error checking online status");
            }
            Ok(ContactOnlineStatus::from_byte(result).expect("A valid u8 from FFI status"))
        }
    }

    fn create_message(&self, receiver: &TariAddress, message: String) -> Result<Message, ClientError> {
        let receiver_address_ptr = Box::into_raw(Box::new(receiver.to_owned())) as *mut FfiTariAddress;
        let sender_address_ptr = Box::into_raw(Box::new(self.address.clone())) as *mut FfiTariAddress;

        let message_c_str = CString::new(message).unwrap();
        let message_c_char: *const c_char = CString::into_raw(message_c_str) as *const c_char;

        let error_out = Box::into_raw(Box::new(0));

        unsafe {
            let message_ptr = create_chat_message(receiver_address_ptr, sender_address_ptr, message_c_char, error_out)
                as *mut Message;
            if *error_out != 0 {
                panic!("Error creating message");
            }
            Ok(*Box::from_raw(message_ptr))
        }
    }

    async fn get_messages(&self, address: &TariAddress, limit: u64, page: u64) -> Result<Vec<Message>, ClientError> {
        let client = self.ptr.lock().unwrap();

        let address_ptr = Box::into_raw(Box::new(address.clone())) as *mut FfiTariAddress;

        let messages;
        unsafe {
            let error_out = Box::into_raw(Box::new(0));
            let limit = i32::try_from(limit).expect("Truncation occurred") as c_uint;
            let page = i32::try_from(page).expect("Truncation occurred") as c_uint;
            let all_messages = get_chat_messages(client.0, address_ptr, limit, page, error_out) as *mut MessagesVector;
            if *error_out != 0 {
                panic!("Error getting messages");
            }
            messages = (*all_messages).0.clone();
        }

        Ok(messages)
    }

    async fn get_message(&self, message_id: &MessageId) -> Result<Message, ClientError> {
        let client = self.ptr.lock().unwrap();

        let error_out = Box::into_raw(Box::new(0));
        let len = i32::try_from(message_id.len()).expect("Truncation occurred") as c_uint;
        let byte_vector = unsafe {
            let byte_vector = chat_byte_vector_create(message_id.as_ptr(), len, error_out);
            if *error_out != 0 {
                panic!("Error creating byte vector");
            }
            byte_vector
        };

        unsafe {
            let message = get_chat_message(client.0, byte_vector, error_out) as *mut Message;
            if *error_out != 0 {
                panic!("Error getting message");
            }
            let message = (*message).clone();
            return Ok(message);
        }
    }

    async fn send_message(&self, message: Message) -> Result<(), ClientError> {
        let client = self.ptr.lock().unwrap();

        let error_out = Box::into_raw(Box::new(0));
        let message_ptr = Box::into_raw(Box::new(message)) as *mut c_void;

        unsafe {
            send_chat_message(client.0, message_ptr, error_out);
            if *error_out != 0 {
                panic!("Error sending message");
            }
        }

        Ok(())
    }

    async fn send_read_receipt(&self, message: Message) -> Result<(), ClientError> {
        let client = self.ptr.lock().unwrap();
        let message_ptr = Box::into_raw(Box::new(message)) as *mut c_void;
        let error_out = Box::into_raw(Box::new(0));

        unsafe {
            send_read_confirmation_for_message(client.0, message_ptr, error_out);
            if *error_out != 0 {
                panic!("Error sending read receipt");
            }
        }

        Ok(())
    }

    async fn get_conversationalists(&self) -> Result<Vec<TariAddress>, ClientError> {
        let client = self.ptr.lock().unwrap();

        let addresses;
        unsafe {
            let error_out = Box::into_raw(Box::new(0));
            let vector = get_conversationalists(client.0, error_out) as *mut Conversationalists;
            if *error_out != 0 {
                panic!("Error getting conversationalists");
            }
            addresses = (*vector).0.clone();
        }

        Ok(addresses)
    }

    fn address(&self) -> TariAddress {
        self.address.clone()
    }

    fn shutdown(&mut self) {
        let client = self.ptr.lock().unwrap();

        unsafe { destroy_chat_client(client.0) }
    }
}

pub async fn spawn_ffi_chat_client(name: &str, seed_peers: Vec<Peer>, base_dir: PathBuf) -> ChatFFI {
    let port = get_port(18000..18499).unwrap();
    let address = Multiaddr::from_str(&format!("/ip4/127.0.0.1/tcp/{}", port)).unwrap();

    let base_dir = base_dir
        .join("ffi_chat_clients")
        .join(format!("{}_port_{}", name, port));

    let mut config = test_config(address.clone());
    config.chat_client.set_base_path(base_dir);

    let identity = setup_node_identity(
        &config.chat_client.identity_file,
        vec![address],
        true,
        PeerFeatures::COMMUNICATION_NODE,
    )
    .unwrap();

    database::create_chat_storage(&config.chat_client.db_file).unwrap();
    database::create_peer_storage(&config.chat_client.data_dir).unwrap();

    config.peer_seeds.peer_seeds = seed_peers
        .iter()
        .map(|p| p.to_short_string())
        .collect::<Vec<String>>()
        .into();

    let config_ptr = Box::into_raw(Box::new(config)) as *mut c_void;

    let client_ptr;

    let error_out = Box::into_raw(Box::new(0));
    let address =
        TariAddress::new_single_address_with_interactive_only(identity.public_key().clone(), Network::LocalNet);
    let address_ptr = Box::into_raw(Box::new(address.clone())) as *mut c_void;
    unsafe {
        *ChatCallback::instance().contact_status_change.lock().unwrap() = 0;

        client_ptr = create_chat_client(
            config_ptr,
            callback_contact_status_change,
            callback_message_received,
            callback_delivery_confirmation_received,
            callback_read_confirmation_received,
            address_ptr,
            error_out,
        );
        if *error_out != 0 {
            panic!("Error creating chat client");
        }
    }

    ChatFFI {
        ptr: Arc::new(Mutex::new(PtrWrapper(client_ptr))),
        address,
    }
}

pub async fn sideload_ffi_chat_client(
    address: TariAddress,
    base_dir: PathBuf,
    contacts_handle_ptr: *mut c_void,
) -> ChatFFI {
    let mut config = test_config(Multiaddr::empty());
    config.chat_client.set_base_path(base_dir);

    let config_ptr = Box::into_raw(Box::new(config)) as *mut c_void;
    let adress_ptr = Box::into_raw(Box::new(address.clone())) as *mut c_void;

    let client_ptr;
    let error_out = Box::into_raw(Box::new(0));
    unsafe {
        *ChatCallback::instance().contact_status_change.lock().unwrap() = 0;

        client_ptr = sideload_chat_client(
            config_ptr,
            contacts_handle_ptr,
            callback_contact_status_change,
            callback_message_received,
            callback_delivery_confirmation_received,
            callback_read_confirmation_received,
            adress_ptr,
            error_out,
        );
        if *error_out != 0 {
            panic!("Error sideloading chat client");
        }
    }

    ChatFFI {
        ptr: Arc::new(Mutex::new(PtrWrapper(client_ptr))),
        address,
    }
}
static mut INSTANCE: Option<ChatCallback> = None;
static START: Once = Once::new();

#[derive(Default)]
pub struct ChatCallback {
    pub contact_status_change: Mutex<u64>,
    pub message_received: Mutex<u64>,
    pub delivery_confirmation_received: Mutex<u64>,
    pub read_confirmation_received: Mutex<u64>,
}

impl ChatCallback {
    pub fn instance() -> &'static mut Self {
        unsafe {
            START.call_once(|| {
                INSTANCE = Some(Self::default());
            });
            INSTANCE.as_mut().unwrap()
        }
    }
}
