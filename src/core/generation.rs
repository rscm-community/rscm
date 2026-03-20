use crate::store::Store;
use crate::store::generation::Generation;
use anyhow::Result;
use crate::config::Configuration;

pub struct GenerationManager {
    store: Store,
}

impl GenerationManager {
    pub fn build(&self, config: &Configuration) -> Result<u64>{
        todo!()
    }

    pub fn switch(&self, id: u64) -> Result<()>{
        todo!()
    }

    pub fn list(&self) -> Result<Vec<Generation>>{
        todo!()
    }
    pub fn delete(&self, id: u64) -> Result<()>{
        todo!()
    }

    pub fn gc(&self) -> Result<()> {
        todo!()
    }
}
