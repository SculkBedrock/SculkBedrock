use crate::connection::controller::window::ReliableWindow;

pub mod window;

pub struct Controller {
    pub window: ReliableWindow,
}
