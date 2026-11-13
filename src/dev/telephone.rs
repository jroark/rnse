//! Host-side state of a simulated telephone.
//!
//! This is independent of the firmware wire protocol. It deliberately
//! emits no CAN frames until that protocol has been established; changing
//! this state alone does not make the firmware recognize a telephone.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contact {
    pub name: String,
    pub number: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    Idle,
    Ringing(String),
    Dialing(String),
    Active(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub enabled: bool,
    pub connected: bool,
    pub call: Call,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Enable(bool),
    Connect,
    Disconnect,
    Incoming(String),
    Dial(String),
    DialContact(usize),
    Answer,
    Reject,
    RemoteAnswer,
    HangUp,
    RemoteHangUp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Disabled,
    Disconnected,
    Busy,
    NoIncomingCall,
    NoOutgoingCall,
    NoCall,
    InvalidNumber,
    UnknownContact,
    ScheduleFull,
    TimePassed,
}

/// A single phone and a single call. No host hardware or wall clock is used.
pub struct Telephone {
    status: Status,
    contacts: Vec<Contact>,
    events: Vec<(u64, Action)>,
    now_ms: u64,
}

impl Default for Telephone {
    fn default() -> Self {
        Self {
            status: Status { enabled: false, connected: false, call: Call::Idle },
            contacts: sample_contacts(),
            events: Vec::new(),
            now_ms: 0,
        }
    }
}

fn sample_contacts() -> Vec<Contact> {
    [("Alex Morgan", "+12025550101"), ("Jamie Chen", "+12025550102"),
        ("Sam Rivera", "+12025550103")].into_iter()
        .map(|(name, number)| Contact { name: name.into(), number: number.into() }).collect()
}

fn valid_number(number: &str) -> bool {
    let digits = number.strip_prefix('+').unwrap_or(number);
    !digits.is_empty() && number.len() <= 32
        && digits.bytes().all(|b| b.is_ascii_digit() || b == b'*' || b == b'#')
}

impl Telephone {
    pub fn status(&self) -> &Status { &self.status }
    pub fn contacts(&self) -> &[Contact] { &self.contacts }

    /// Invalid actions leave the entire state unchanged.
    pub fn apply(&mut self, action: Action) -> Result<(), Error> {
        match action {
            Action::Enable(enabled) => {
                if self.status.enabled == enabled { return Ok(()); }
                self.status = Status { enabled, connected: enabled, call: Call::Idle };
                return Ok(());
            }
            _ if !self.status.enabled => return Err(Error::Disabled),
            Action::Connect => {
                self.status.connected = true;
                return Ok(());
            }
            Action::Disconnect => {
                self.status.connected = false;
                self.status.call = Call::Idle;
                return Ok(());
            }
            _ if !self.status.connected => return Err(Error::Disconnected),
            Action::Incoming(number) => {
                if !valid_number(&number) { return Err(Error::InvalidNumber); }
                if self.status.call != Call::Idle { return Err(Error::Busy); }
                self.status.call = Call::Ringing(number);
            }
            Action::Dial(number) => {
                if !valid_number(&number) { return Err(Error::InvalidNumber); }
                if self.status.call != Call::Idle { return Err(Error::Busy); }
                self.status.call = Call::Dialing(number);
            }
            Action::DialContact(index) => {
                let number = self.contacts.get(index).ok_or(Error::UnknownContact)?.number.clone();
                if self.status.call != Call::Idle { return Err(Error::Busy); }
                self.status.call = Call::Dialing(number);
            }
            Action::Answer => match &self.status.call {
                Call::Ringing(number) => self.status.call = Call::Active(number.clone()),
                _ => return Err(Error::NoIncomingCall),
            },
            Action::RemoteAnswer => match &self.status.call {
                Call::Dialing(number) => self.status.call = Call::Active(number.clone()),
                _ => return Err(Error::NoOutgoingCall),
            },
            Action::Reject => match self.status.call {
                Call::Ringing(_) => self.status.call = Call::Idle,
                _ => return Err(Error::NoIncomingCall),
            },
            Action::HangUp | Action::RemoteHangUp => {
                if self.status.call == Call::Idle { return Err(Error::NoCall); }
                self.status.call = Call::Idle;
            }
        }
        Ok(())
    }

    /// Queue an action at an absolute emulated time. Equal times retain
    /// insertion order; the bounded queue also limits accidental input floods.
    pub fn schedule(&mut self, at_ms: u64, action: Action) -> Result<(), Error> {
        if at_ms < self.now_ms { return Err(Error::TimePassed); }
        if self.events.len() >= 1024 { return Err(Error::ScheduleFull); }
        let index = self.events.partition_point(|(time, _)| *time <= at_ms);
        self.events.insert(index, (at_ms, action));
        Ok(())
    }

    /// Advance using emulated milliseconds. Return outcomes so callers can
    /// report invalid scripted transitions rather than silently ignoring them.
    pub fn advance(&mut self, now_ms: u64) -> Result<Vec<(u64, Result<(), Error>)>, Error> {
        if now_ms < self.now_ms { return Err(Error::TimePassed); }
        self.now_ms = now_ms;
        let count = self.events.partition_point(|(time, _)| *time <= now_ms);
        let due: Vec<_> = self.events.drain(..count).collect();
        Ok(due.into_iter().map(|(time, action)| (time, self.apply(action))).collect())
    }

    /// A browser restart retains connection settings but clears transient
    /// calls, pending events and the emulated clock.
    pub fn restart(&mut self) {
        self.status.call = Call::Idle;
        self.contacts = sample_contacts();
        self.events.clear();
        self.now_ms = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connected() -> Telephone {
        let mut phone = Telephone::default();
        phone.apply(Action::Enable(true)).unwrap();
        phone
    }

    #[test]
    fn incoming_call_lifecycle_and_busy_rejection() {
        let mut p = connected();
        p.apply(Action::Incoming("+12025550101".into())).unwrap();
        let before = p.status().clone();
        assert_eq!(p.apply(Action::DialContact(1)), Err(Error::Busy));
        assert_eq!(p.status(), &before);
        p.apply(Action::Answer).unwrap();
        assert_eq!(p.status().call, Call::Active("+12025550101".into()));
        p.apply(Action::RemoteHangUp).unwrap();
        assert_eq!(p.status().call, Call::Idle);
        p.apply(Action::Incoming("123".into())).unwrap();
        p.apply(Action::Reject).unwrap();
        assert_eq!(p.status().call, Call::Idle);
    }

    #[test]
    fn contacts_dial_remote_answer_and_disconnect() {
        let mut p = connected();
        assert_eq!(p.contacts().len(), 3);
        p.apply(Action::DialContact(1)).unwrap();
        let number = p.contacts()[1].number.clone();
        assert_eq!(p.status().call, Call::Dialing(number.clone()));
        p.apply(Action::RemoteAnswer).unwrap();
        assert_eq!(p.status().call, Call::Active(number));
        p.apply(Action::Disconnect).unwrap();
        assert_eq!(p.status().call, Call::Idle);
        assert_eq!(p.apply(Action::DialContact(0)), Err(Error::Disconnected));
        p.apply(Action::Connect).unwrap();
        assert_eq!(p.contacts().len(), 3);
    }

    #[test]
    fn direct_dial_local_hangup_and_disable_during_call() {
        let mut p = connected();
        p.apply(Action::Dial("*123#".into())).unwrap();
        assert_eq!(p.status().call, Call::Dialing("*123#".into()));
        p.apply(Action::Enable(true)).unwrap();
        assert_eq!(p.status().call, Call::Dialing("*123#".into()));
        p.apply(Action::RemoteAnswer).unwrap();
        p.apply(Action::HangUp).unwrap();
        p.apply(Action::Incoming("123".into())).unwrap();
        p.apply(Action::Enable(false)).unwrap();
        assert_eq!(p.status(), &Status { enabled: false, connected: false, call: Call::Idle });
        p.apply(Action::Enable(true)).unwrap();
        assert!(p.status().connected);
        assert_eq!(p.status().call, Call::Idle);
    }

    #[test]
    fn scheduling_is_stable_bounded_and_uses_emulated_time() {
        let mut p = Telephone::default();
        p.schedule(100, Action::Enable(true)).unwrap();
        p.schedule(100, Action::Dial("123".into())).unwrap();
        p.schedule(200, Action::RemoteAnswer).unwrap();
        assert!(p.advance(99).unwrap().is_empty());
        assert_eq!(p.advance(100).unwrap(), vec![(100, Ok(())), (100, Ok(()))]);
        assert_eq!(p.status().call, Call::Dialing("123".into()));
        assert_eq!(p.advance(200).unwrap(), vec![(200, Ok(()))]);
        assert_eq!(p.schedule(199, Action::Disconnect), Err(Error::TimePassed));
        assert_eq!(p.advance(199), Err(Error::TimePassed));
        for _ in 0..1024 { p.schedule(300, Action::Disconnect).unwrap(); }
        assert_eq!(p.schedule(300, Action::Disconnect), Err(Error::ScheduleFull));
        p.restart();
        assert_eq!(p.status().call, Call::Idle);
        assert!(p.status().connected);
        assert!(p.advance(300).unwrap().is_empty());
    }

    #[test]
    fn invalid_actions_do_not_change_state() {
        let mut p = connected();
        for (action, error) in [
            (Action::Dial("".into()), Error::InvalidNumber),
            (Action::Incoming("12\n3".into()), Error::InvalidNumber),
            (Action::DialContact(3), Error::UnknownContact),
            (Action::Answer, Error::NoIncomingCall),
            (Action::Reject, Error::NoIncomingCall),
            (Action::RemoteAnswer, Error::NoOutgoingCall),
            (Action::HangUp, Error::NoCall),
        ] {
            let before = p.status().clone();
            assert_eq!(p.apply(action), Err(error));
            assert_eq!(p.status(), &before);
        }
        p.apply(Action::Enable(false)).unwrap();
        assert_eq!(p.apply(Action::Connect), Err(Error::Disabled));
    }
}
