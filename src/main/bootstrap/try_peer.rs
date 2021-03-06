// Copyright 2016 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under (1) the MaidSafe.net Commercial License,
// version 1.0 or later, or (2) The General Public License (GPL), version 3, depending on which
// licence you accepted on initial access to the Software (the "Licences").
//
// By contributing code to the SAFE Network Software, or to this project generally, you agree to be
// bound by the terms of the MaidSafe Contributor Agreement, version 1.0.  This, along with the
// Licenses can be found in the root directory of this project at LICENSE, COPYING and CONTRIBUTOR.
//
// Unless required by applicable law or agreed to in writing, the SAFE Network Software distributed
// under the GPL Licence is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.
//
// Please review the Licences for the specific language governing permissions and limitations
// relating to use of the SAFE Network Software.

use common::{BootstrapDenyReason, Core, ExternalReachability, Message, NameHash, Priority, Socket,
             State};
use main::PeerId;
use mio::{EventLoop, EventSet, PollOpt, Token};
use rust_sodium::crypto::box_::PublicKey;
use std::any::Any;
use std::cell::RefCell;
use std::mem;
use std::net::SocketAddr;
use std::rc::Rc;

// TODO(Spandan) Result contains socket address too as currently due to bug in mio we are unable to
// obtain peer address from a connected socket. Track https://github.com/carllerche/mio/issues/397
// and remove this once that is solved.
pub type Finish = Box<FnMut(&mut Core,
                            &mut EventLoop<Core>,
                            Token,
                            Result<(Socket, SocketAddr, PeerId),
                                   (SocketAddr, Option<BootstrapDenyReason>)>)>;

pub struct TryPeer {
    token: Token,
    peer: SocketAddr,
    socket: Socket,
    request: Option<(Message, Priority)>,
    finish: Finish,
}

impl TryPeer {
    pub fn start(core: &mut Core,
                 el: &mut EventLoop<Core>,
                 peer: SocketAddr,
                 our_pk: PublicKey,
                 name_hash: NameHash,
                 ext_reachability: ExternalReachability,
                 finish: Finish)
                 -> ::Res<Token> {
        let socket = Socket::connect(&peer)?;
        let token = core.get_new_token();

        el.register(&socket,
                      token,
                      EventSet::error() | EventSet::hup() | EventSet::writable(),
                      PollOpt::edge())?;

        let state = TryPeer {
            token: token,
            peer: peer,
            socket: socket,
            request: Some((Message::BootstrapRequest(our_pk, name_hash, ext_reachability), 0)),
            finish: finish,
        };

        let _ = core.insert_state(token, Rc::new(RefCell::new(state)));

        Ok(token)
    }

    fn write(&mut self,
             core: &mut Core,
             el: &mut EventLoop<Core>,
             msg: Option<(Message, Priority)>) {
        if self.socket.write(el, self.token, msg).is_err() {
            self.handle_error(core, el, None);
        }
    }

    fn read(&mut self, core: &mut Core, el: &mut EventLoop<Core>) {
        match self.socket.read::<Message>() {
            Ok(Some(Message::BootstrapGranted(peer_pk))) => {
                let _ = core.remove_state(self.token);
                let token = self.token;
                let socket = mem::replace(&mut self.socket, Socket::default());
                let data = (socket, self.peer, PeerId(peer_pk));
                (*self.finish)(core, el, token, Ok(data));
            }
            Ok(Some(Message::BootstrapDenied(reason))) => self.handle_error(core, el, Some(reason)),
            Ok(None) => (),
            Ok(Some(_)) | Err(_) => self.handle_error(core, el, None),
        }
    }

    fn handle_error(&mut self,
                    core: &mut Core,
                    el: &mut EventLoop<Core>,
                    reason: Option<BootstrapDenyReason>) {
        self.terminate(core, el);
        let token = self.token;
        let peer = self.peer;
        (*self.finish)(core, el, token, Err((peer, reason)));
    }
}

impl State for TryPeer {
    fn ready(&mut self, core: &mut Core, el: &mut EventLoop<Core>, es: EventSet) {
        if es.is_error() || es.is_hup() {
            self.handle_error(core, el, None);
        } else {
            if es.is_writable() {
                let req = self.request.take();
                self.write(core, el, req);
            }
            if es.is_readable() {
                self.read(core, el)
            }
        }
    }

    fn terminate(&mut self, core: &mut Core, el: &mut EventLoop<Core>) {
        let _ = core.remove_state(self.token);
        let _ = el.deregister(&self.socket);
    }

    fn as_any(&mut self) -> &mut Any {
        self
    }
}
