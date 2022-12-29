use ethereum_forkid::ForkId;
use fastrlp::{Decodable, RlpEncodable};
use rlp::Encodable;


// struct FastRlpEncodableAdapter<T>
// where
//     T: RlpEncodeable
//  {
//   encodable: RlpEncodeable
// }

// impl Encodable for FastRlpEncodableAdapter<T> {

//     fn rlp_append(&self, s: &mut rlp::RlpStream) {
//       // s.append(self.fork_id.)
//     }
// }


pub struct ForkIdEncodableAdapter (pub ForkId);

impl Encodable for ForkIdEncodableAdapter {

    fn rlp_append(&self, s: &mut rlp::RlpStream) {


      let hash = self.0.hash;

      s.append_list(&hash.0.clone());
      s.append(&self.0.next);

      // let x = self.fork_id; 
      // let encoded = x.encode();
      // let y: &dyn RlpEncodable = &x;
      
    
    }
}



// impl Decodable for ForkIdEncodableAdapter {

//     fn decode(buf: &mut &[u8]) -> Result<Self, fastrlp::DecodeError> {
        
//       let result = ForkId{ hash: buf[0..4], next: u64::from_be_bytes(buf[4..12]);
//     }
// }

