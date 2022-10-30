use std::sync::Weak;

use ethcore::client::ReservedPeersManagement;



pub(crate) struct ReservedPeersWrapper {
  manage_network: Weak<dyn sync::ManageNetwork>,
}

impl ReservedPeersWrapper {
  pub fn new(manage_network: Weak<dyn sync::ManageNetwork>) -> Self {
    ReservedPeersWrapper { manage_network }
  }
}

impl ReservedPeersManagement for ReservedPeersWrapper {
  fn add_reserved_peer(&self, peer: String) -> Result<(), String> {
      match self.manage_network.upgrade() {
          Some(sync_arc) => sync_arc.add_reserved_peer(peer),
          None => Err("ManageNetwork instance not available.".to_string()),
      }
  }
}
