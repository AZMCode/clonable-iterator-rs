#![feature(once_cell)]
#![feature(cell_update)]

//! Dependency-Free Iterator Extension Trait with accompanying struct to make !Clone Iterators with Clone elements into a Clone Iterator 

mod symbol {
    use std::lazy::SyncLazy;
    use std::sync::Mutex;
    use std::cell::Cell;
    #[derive(Default, Clone, Copy)]
    struct CounterStruct([u128;16]);
    
    static GLOBAL_SYMBOL_COUNTER: SyncLazy<Mutex<Cell<CounterStruct>>> = SyncLazy::new(|| Mutex::new(Cell::new(CounterStruct::default())));

    impl CounterStruct {
        fn get_and_add_one(&mut self) -> [u128;16] {
            let mut add_to_position = Some(0);
            let mut last_pos_fetched = 0;
            let mut output = [0;16];
            while let Some(pos) = add_to_position {
                match (self.0)[pos] {
                    ref mut val if *val == u128::MAX => {
                        output[pos] = *val;
                        *val = 0;
                        add_to_position = Some(pos + 1);
                    },
                    ref mut val => {
                        output[pos] = *val;
                        *val += 1;
                        last_pos_fetched = pos;
                        add_to_position = None;
                    }
                }
            }
            for pos in (last_pos_fetched + 1)..output.len() {
                output[pos] = (self.0)[pos];
                (self.0)[pos] += 1
            }
            output
        }
    }

    #[derive(Debug,Hash,PartialEq, Eq, Clone)]
    pub struct Symbol([u128;16]);

    impl Symbol {
        pub fn new() -> Self {
            let mut output = [0;16];
            GLOBAL_SYMBOL_COUNTER.lock().unwrap().update(|mut elm| {
                output = elm.get_and_add_one();
                elm
            });
            Symbol(output)
        }
    }
}

mod bus {
    use std::sync::mpsc;
    use std::collections::HashMap;
    use super::symbol::Symbol;

    pub struct Bus<T: Clone> {
        senders: HashMap<Symbol,mpsc::Sender<T>>
    }

    impl<T: Clone> Default for Bus<T> {
        fn default() -> Self {
            Bus { senders: HashMap::new() }
        }
    }

    impl<T: Clone> Bus<T> {
        pub fn broadcast(&mut self, val: T) -> Result<(),Vec<mpsc::SendError<T>>> {
            let mut errors = vec![];
            let mut iter = self.senders.values().peekable();
            let mut last_sender = None;
            while let Some(sender) = iter.next() {
                match iter.peek() {
                    Some(_) => match sender.send(val.clone()) {
                        Ok(_) => (),
                        Err(e) => errors.push(e)
                    },
                    None => {
                        last_sender = Some(sender)
                    }
                };
            }
            match last_sender {
                Some(sender) => match sender.send(val.clone()) {
                    Ok(_) => (),
                    Err(e) => errors.push(e)
                },
                None => ()
            };
            if errors.len() > 0 {
                Err(errors)
            } else {
                Ok(())
            }
        }
        pub fn add_rx(&mut self) -> BusReceiver<T> {
            let (new_sender,new_receiver) = mpsc::channel();
            let new_id = Symbol::new();
            self.senders.insert(new_id.clone(), new_sender);
            BusReceiver {
                receiver: new_receiver,
                id: new_id
            }
        }
        pub fn branch_off(&mut self, prev: &BusReceiver<T>) -> BusReceiver<T> {
            let (new_sender,new_receiver) = mpsc::channel();
            let new_id = Symbol::new();

            let mut stored_items = vec![];
            for item in prev.receiver.try_iter() {
                stored_items.push(item.clone());
                new_sender.send(item).unwrap();
            }
            let prev_sender = self.senders.get_mut(&prev.id).unwrap();
            for item in stored_items {
                prev_sender.send(item).unwrap();
            }
            self.senders.insert(new_id.clone(), new_sender);

            BusReceiver {
                receiver: new_receiver,
                id: new_id
            }
        }
    }

    pub struct BusReceiver<T: Clone> {
        receiver: mpsc::Receiver<T>,
        id: Symbol
    }

    impl<T: Clone> BusReceiver<T> {
        pub fn try_recv(&self) -> Result<T,mpsc::TryRecvError> {
            self.receiver.try_recv()
        }
    }
}
mod clonable_iterator {
    use std::sync::Mutex;
    use std::sync::Arc;
    struct ClonableIteratorOwner<T: Iterator> where T::Item: Clone{
        inner: T,
        sender: super::bus::Bus<T::Item>
    }
    impl<T: Iterator> ClonableIteratorOwner<T> where T::Item: Clone{
        fn produce(&mut self) {
            match self.inner.next() {
                Some(val) => { self.sender.broadcast(val).ok(); },
                None => ()
            }
        }
    }
    /// A Clonable wrapper for any Iterator
    /// Any Iterator can be made into a Clonable, as long as the items they produce are themselves Clone.
    /// Note that the Iterator itself doesn't need to be Clone.
    /// 
    /// The clones of this iterator will start wherever the parent iterator left off, and the
    /// parent iterator will not be affected by this.
    /// 
    /// The original Iterator will be held in a Mutex, for concurrent access, which will itself be held
    /// inside an Arc, for shared ownership. The Original Iterator will be dropped whenever all clones
    /// stemming from it.
    pub struct Clonable<T: Iterator> where T::Item: Clone {
        owner: Arc<Mutex<ClonableIteratorOwner<T>>>,
        receiver: super::bus::BusReceiver<T::Item>
    }

    impl<T: Iterator> Clonable<T> where T::Item: Clone {
        fn new(inner: T) -> Self {
            let mut sender = super::bus::Bus::default();
            let receiver = sender.add_rx();
            Clonable {
                owner: Arc::new(Mutex::new(ClonableIteratorOwner {
                    inner,
                    sender
                })),
                receiver
            }
        }
    }

    impl<T: Iterator> Clone for Clonable<T> where T::Item: Clone {
        fn clone(&self) -> Self {
            let receiver = self.owner.lock().unwrap().sender.branch_off(&self.receiver);
            Clonable {
                owner: Arc::clone(&self.owner),
                receiver
            }
        }
    }

    /// Extension trait on all Iterators that adds a single `clonable` method
    pub trait IterExt: Iterator + Sized where Self::Item: Clone {
        /// This method consumes the iterator and produces a Clonable
        fn clonable(self) -> Clonable<Self> {
            Clonable::new(self)
        }
    }

    impl<T: Iterator> IterExt for T where T::Item: Clone{}

    impl<T: Iterator> Iterator for Clonable<T> where T::Item: Clone {
        type Item = T::Item;
        fn next(&mut self) -> Option<Self::Item> {
            match self.receiver.try_recv() {
                Ok(val) => Some(val),
                Err(_) => {
                    self.owner.lock().unwrap().produce();
                    match self.receiver.try_recv() {
                        Ok(val) => Some(val),
                        Err(_) => None
                    }
                }
            }
        }
    }
}
pub use self::clonable_iterator::IterExt;
pub use self::clonable_iterator::Clonable;
