use rumqttc::{ AsyncClient, Event, MqttOptions, Packet, QoS };
use tokio::sync::mpsc;
use tokio::time::{ sleep, Duration };
use crossterm::{
    event::{ self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers },
    terminal::{ self, size },
};
use serde::{ Deserialize, Serialize };
use std::io::{ self, Write };
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "mqtt-shell-controller")]
#[command(about = "MQTT Shell Controller - Terminal client for remote shell access")]
struct Args {
    #[arg(short, long, default_value = "shell")]
    channel: String,

    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    #[arg(long, default_value_t = 1883)]
    port: u16,
}

#[derive(Serialize, Deserialize, Debug)]
struct TerminalResize {
    rows: u16,
    cols: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    println!("Starting MQTT Shell Controller with TTY support...");
    println!("📡 Using channel: '{}' at {}:{}", args.channel, args.host, args.port);

    let shell_in = format!("{}/in", args.channel);
    let shell_out = format!("{}/out", args.channel);
    let shell_status = format!("{}/status", args.channel);
    let shell_resize = format!("{}/resize", args.channel);

    let mut mqttoptions = MqttOptions::new("controller", &args.host, args.port);
    mqttoptions.set_keep_alive(Duration::from_secs(5));
    let (client, mut eventloop) = AsyncClient::new(mqttoptions, 10);

    client.subscribe(&shell_out, QoS::AtMostOnce).await.unwrap();
    client.subscribe(&shell_status, QoS::AtMostOnce).await.unwrap();
    println!("🔍 Controller subscribed to {} and {}", shell_out, shell_status);

    let (cols, rows) = size().unwrap_or((80, 24));
    let initial_size = TerminalResize { rows, cols };

    let size_json = serde_json::to_string(&initial_size)?;
    client.publish(&shell_resize, QoS::AtMostOnce, false, size_json).await?;

    println!("Controller connected. Terminal size: {}x{}", cols, rows);
    println!("Press Ctrl+Q to exit.");
    println!("You can now use editors like nano, vim, etc.");

    terminal::enable_raw_mode()?;

    let (tx_input, mut rx_input) = mpsc::unbounded_channel::<Vec<u8>>();
    let (tx_exit, mut rx_exit) = mpsc::unbounded_channel::<()>();
    let client_input = client.clone();

    tokio::spawn(async move {
        while let Some(input) = rx_input.recv().await {
            if let Err(e) = client_input.publish(&shell_in, QoS::AtMostOnce, false, input).await {
                eprintln!("Error sending input: {:?}", e);
            }
        }
    });

    let tx_exit_clone = tx_exit.clone();
    tokio::spawn(async move {
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::Publish(p))) => {
                    match p.topic.as_str() {
                        topic if topic == shell_out => {
                            print!("{}", String::from_utf8_lossy(&p.payload));
                            let _ = io::stdout().flush();
                        }
                        topic if topic == shell_status => {
                            let status = String::from_utf8_lossy(&p.payload);
                            println!("📡 Status received: '{}'", status);
                            if status == "shell_exited" {
                                println!("🎉 Received shell_exited - sending exit signal");
                                match tx_exit_clone.send(()) {
                                    Ok(_) => println!("✅ Exit signal sent successfully"),
                                    Err(e) => println!("❌ Error sending exit signal: {:?}", e),
                                }
                                break;
                            } else {
                                println!("🔍 Status ignored: '{}'", status);
                            }
                        }
                        _ => {
                            println!("❓ Unknown topic: '{}'", p.topic);
                        }
                    }
                }
                Ok(Event::Incoming(Packet::ConnAck(_))) => {}
                Ok(_) => {}
                Err(e) => {
                    eprintln!("MQTT Error: {:?}", e);
                    sleep(Duration::from_secs(1)).await;
                }
            }
        }
    });

    let client_resize = client.clone();
    tokio::spawn(async move {
        let mut last_size = (cols, rows);
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if let Ok((new_cols, new_rows)) = size() {
                if (new_cols, new_rows) != last_size {
                    last_size = (new_cols, new_rows);
                    let resize_data = TerminalResize {
                        rows: new_rows,
                        cols: new_cols,
                    };
                    if let Ok(json) = serde_json::to_string(&resize_data) {
                        let _ = client_resize.publish(
                            &shell_resize,
                            QoS::AtMostOnce,
                            false,
                            json
                        ).await;
                    }
                }
            }
        }
    });

    println!("🔍 Main loop started - press Ctrl+Q to exit manually");
    loop {
        if rx_exit.try_recv().is_ok() {
            println!("\r\n🎉 Exit signal received from remote shell");
            break;
        }

        if event::poll(Duration::from_millis(10))? {
            match event::read()? {
                CrosstermEvent::Key(
                    KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::CONTROL, .. },
                ) => {
                    break;
                }
                CrosstermEvent::Key(
                    KeyEvent { code: KeyCode::Char('c'), modifiers: KeyModifiers::CONTROL, .. },
                ) => {
                    let _ = tx_input.send(vec![3]);
                }
                CrosstermEvent::Key(
                    KeyEvent { code: KeyCode::Char('z'), modifiers: KeyModifiers::CONTROL, .. },
                ) => {
                    let _ = tx_input.send(vec![26]);
                }
                CrosstermEvent::Key(
                    KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::NONE, .. },
                ) => {
                    let mut bytes = [0u8; 4];
                    let encoded = c.encode_utf8(&mut bytes);
                    let _ = tx_input.send(encoded.bytes().collect());
                }
                CrosstermEvent::Key(
                    KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::SHIFT, .. },
                ) => {
                    let mut bytes = [0u8; 4];
                    let encoded = c.encode_utf8(&mut bytes);
                    let _ = tx_input.send(encoded.bytes().collect());
                }
                CrosstermEvent::Key(KeyEvent { code: KeyCode::Enter, .. }) => {
                    let _ = tx_input.send(vec![b'\r']);
                }
                CrosstermEvent::Key(KeyEvent { code: KeyCode::Backspace, .. }) => {
                    let _ = tx_input.send(vec![127]);
                }
                CrosstermEvent::Key(KeyEvent { code: KeyCode::Tab, .. }) => {
                    let _ = tx_input.send(vec![b'\t']);
                }
                CrosstermEvent::Key(KeyEvent { code: KeyCode::Up, .. }) => {
                    let _ = tx_input.send(b"\x1b[A".to_vec());
                }
                CrosstermEvent::Key(KeyEvent { code: KeyCode::Down, .. }) => {
                    let _ = tx_input.send(b"\x1b[B".to_vec());
                }
                CrosstermEvent::Key(KeyEvent { code: KeyCode::Right, .. }) => {
                    let _ = tx_input.send(b"\x1b[C".to_vec());
                }
                CrosstermEvent::Key(KeyEvent { code: KeyCode::Left, .. }) => {
                    let _ = tx_input.send(b"\x1b[D".to_vec());
                }
                CrosstermEvent::Key(KeyEvent { code: KeyCode::Home, .. }) => {
                    let _ = tx_input.send(b"\x1b[H".to_vec());
                }
                CrosstermEvent::Key(KeyEvent { code: KeyCode::End, .. }) => {
                    let _ = tx_input.send(b"\x1b[F".to_vec());
                }
                CrosstermEvent::Key(KeyEvent { code: KeyCode::PageUp, .. }) => {
                    let _ = tx_input.send(b"\x1b[5~".to_vec());
                }
                CrosstermEvent::Key(KeyEvent { code: KeyCode::PageDown, .. }) => {
                    let _ = tx_input.send(b"\x1b[6~".to_vec());
                }
                CrosstermEvent::Key(KeyEvent { code: KeyCode::Delete, .. }) => {
                    let _ = tx_input.send(b"\x1b[3~".to_vec());
                }
                CrosstermEvent::Key(KeyEvent { code: KeyCode::Insert, .. }) => {
                    let _ = tx_input.send(b"\x1b[2~".to_vec());
                }
                CrosstermEvent::Key(KeyEvent { code: KeyCode::F(n), .. }) => {
                    let seq = match n {
                        1 => b"\x1bOP".to_vec(),
                        2 => b"\x1bOQ".to_vec(),
                        3 => b"\x1bOR".to_vec(),
                        4 => b"\x1bOS".to_vec(),
                        5 => b"\x1b[15~".to_vec(),
                        6 => b"\x1b[17~".to_vec(),
                        7 => b"\x1b[18~".to_vec(),
                        8 => b"\x1b[19~".to_vec(),
                        9 => b"\x1b[20~".to_vec(),
                        10 => b"\x1b[21~".to_vec(),
                        11 => b"\x1b[23~".to_vec(),
                        12 => b"\x1b[24~".to_vec(),
                        _ => {
                            continue;
                        }
                    };
                    let _ = tx_input.send(seq);
                }
                _ => {}
            }
        }

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    terminal::disable_raw_mode()?;
    println!("\rController disconnected. Terminal restored.");

    Ok(())
}
