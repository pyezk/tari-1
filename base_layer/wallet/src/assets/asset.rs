#[derive(Clone)]
pub struct Asset {
    name : String,
}

impl Asset {
    pub fn new(name: String)  -> Self{
       Self {
           name
       }
    }
}
