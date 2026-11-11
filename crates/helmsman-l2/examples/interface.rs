//! Prints an interface's addresses and says whether raw Ethernet can be
//! opened on it: the first thing to run when a PROFINET device will not
//! answer.
//!
//! ```sh
//! cargo run -p helmsman-l2 --example interface -- en0
//! ```

fn main() {
    let name = std::env::args().nth(1).unwrap_or_else(|| "en0".into());
    match helmsman_l2::addresses(&name) {
        Ok(a) => {
            let mac: Vec<String> = a.mac.iter().map(|b| format!("{b:02x}")).collect();
            let ip = a
                .ipv4
                .map_or("none".into(), |i| std::net::Ipv4Addr::from(i).to_string());
            println!("{name}: mac {} ipv4 {ip}", mac.join(":"));
        }
        Err(e) => {
            println!("{name}: {e}");
            std::process::exit(1);
        }
    }
    match helmsman_l2::RawSocket::open(&name, 0x8892) {
        Ok(_) => println!("raw Ethernet (EtherType 0x8892): can be opened"),
        Err(e) => {
            println!("raw Ethernet (EtherType 0x8892): {e}");
            std::process::exit(2);
        }
    }
}
