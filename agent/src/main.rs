use portable_pty::{ native_pty_system, CommandBuilder, PtyPair, PtySize };
use tokio::sync::broadcast;
use rumqttc::{ AsyncClient, MqttOptions, QoS };
use serde::{ Deserialize, Serialize };
use std::io::{ Read, Write };
use std::sync::{ Arc, Mutex };
use std::time::Duration;
use std::thread;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "mqtt-shell-agent")]
#[command(about = "MQTT Shell Agent - Remote shell access over MQTT")]
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

    println!("🚀 Starting MQTT Shell Agent with auto-reconnect and shell restart...");
    println!("📡 Using channel: '{}' on {}:{}", args.channel, args.host, args.port);

    let topic_in = format!("{}/in", args.channel);
    let topic_out = format!("{}/out", args.channel);
    let topic_status = format!("{}/status", args.channel);
    let topic_resize = format!("{}/resize", args.channel);

    loop {
        let (output_tx, _) = broadcast::channel::<Vec<u8>>(1000);
        let (status_tx, _) = broadcast::channel::<String>(10);
        let (input_tx, input_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let input_rx = Arc::new(Mutex::new(input_rx));
        println!("🔄 Creating new shell instance...");

        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("Failed to open pty");

        let mut cmd = CommandBuilder::new("/bin/bash");
        cmd.arg("-i");
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");

        let mut child = pty_pair.slave.spawn_command(cmd).expect("Failed to spawn shell");

        println!("✅ Shell started in PTY");

        let reader = pty_pair.master.try_clone_reader().expect("Failed to clone reader");
        let writer = pty_pair.master.take_writer().expect("Failed to get writer");
        let writer = Arc::new(Mutex::new(writer));

        let _ = status_tx.send("shell_ready".to_string());

        let output_broadcaster = output_tx.clone();
        let status_broadcaster = status_tx.clone();
        let _ = thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        println!("⚠️  Shell exited - signaling restart");
                        if let Err(e) = status_broadcaster.send("shell_restarting".to_string()) {
                            println!("❌ Failed to send shell_restarting: {:?}", e);
                        } else {
                            println!("✅ shell_restarting signal sent");
                        }
                        break;
                    }
                    Ok(n) => {
                        let _ = output_broadcaster.send(buf[..n].to_vec());
                    }
                    Err(e) => {
                        eprintln!("❌ Error reading PTY: {:?}", e);
                        let _ = status_broadcaster.send("shell_error_restarting".to_string());
                        break;
                    }
                }
            }
        });

        let writer_clone = Arc::clone(&writer);
        let input_rx_clone = Arc::clone(&input_rx);
        let _writer_handle = thread::spawn(move || {
            loop {
                let command = {
                    if let Ok(rx) = input_rx_clone.lock() {
                        rx.recv()
                    } else {
                        break;
                    }
                };
                match command {
                    Ok(cmd) => {
                        if let Ok(mut writer_guard) = writer_clone.lock() {
                            if let Err(e) = writer_guard.write_all(&cmd) {
                                eprintln!("❌ Error writing to PTY: {:?}", e);
                                break;
                            }
                            if let Err(e) = writer_guard.flush() {
                                eprintln!("❌ Error flushing PTY: {:?}", e);
                            }
                        }
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
        });

        let mqtt_task = tokio::spawn({
            let output_tx = output_tx.clone();
            let status_tx = status_tx.clone();
            let input_tx = input_tx.clone();
            let topics = (
                topic_in.clone(),
                topic_out.clone(),
                topic_status.clone(),
                topic_resize.clone(),
            );
            let host = args.host.clone();
            let port = args.port;
            async move {
                mqtt_shell_loop(output_tx, status_tx, input_tx, pty_pair, topics, host, port).await;
            }
        });

        let _ = child.wait();
        println!("🔄 Shell exited, restarting in 2 seconds...");

        mqtt_task.abort();

        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    #[allow(unreachable_code)]
    Ok(())
}

async fn mqtt_shell_loop(
    output_tx: broadcast::Sender<Vec<u8>>,
    status_tx: broadcast::Sender<String>,
    input_tx: std::sync::mpsc::Sender<Vec<u8>>,
    pty_master: PtyPair,
    topics: (String, String, String, String),
    mqtt_host: String,
    mqtt_port: u16
) {
    let (topic_in, topic_out, topic_status, topic_resize) = topics;
    let mut reconnect_delay = 1;

    loop {
        println!("🔌 Connecting to MQTT broker at {}:{}...", mqtt_host, mqtt_port);

        let mut mqttoptions = MqttOptions::new("agent", mqtt_host.clone(), mqtt_port);
        mqttoptions.set_keep_alive(Duration::from_secs(5));
        let (client, mut eventloop) = AsyncClient::new(mqttoptions, 10);

        if let Err(e) = client.subscribe(&topic_in, QoS::AtMostOnce).await {
            eprintln!("❌ Failed to subscribe to {}: {:?}", topic_in, e);
            tokio::time::sleep(Duration::from_secs(reconnect_delay)).await;
            reconnect_delay = std::cmp::min(reconnect_delay * 2, 30);
            continue;
        }

        if let Err(e) = client.subscribe(&topic_resize, QoS::AtMostOnce).await {
            eprintln!("❌ Failed to subscribe to {}: {:?}", topic_resize, e);
            tokio::time::sleep(Duration::from_secs(reconnect_delay)).await;
            reconnect_delay = std::cmp::min(reconnect_delay * 2, 30);
            continue;
        }

        println!("✅ Subscribed to MQTT topics");
        reconnect_delay = 1;

        let mut output_receiver = output_tx.subscribe();
        let mut status_receiver = status_tx.subscribe();
        println!(
            "🔗 Broadcast receivers created (output: {}, status: {})",
            output_tx.receiver_count(),
            status_tx.receiver_count()
        );
        let client_output = client.clone();
        let client_status = client.clone();

        let publish_task = tokio::spawn({
            let topic_out = topic_out.clone();
            async move {
                while let Ok(output) = output_receiver.recv().await {
                    if
                        let Err(_) = client_output.publish(
                            &topic_out,
                            QoS::AtMostOnce,
                            false,
                            output
                        ).await
                    {
                        break;
                    }
                }
            }
        });

        let status_task = tokio::spawn({
            let topic_status = topic_status.clone();
            async move {
                println!("🔍 Status task started");
                while let Ok(status) = status_receiver.recv().await {
                    println!("📤 Publishing status: {}", status);
                    match
                        client_status.publish(
                            &topic_status,
                            QoS::AtMostOnce,
                            false,
                            status.clone()
                        ).await
                    {
                        Ok(_) => println!("✅ Status '{}' published", status),
                        Err(e) => {
                            println!("❌ Failed to publish status '{}': {:?}", status, e);
                            break;
                        }
                    }
                }
                println!("🔍 Status task ended");
            }
        });

        println!("✅ MQTT Agent ready");

        loop {
            match eventloop.poll().await {
                Ok(rumqttc::Event::Incoming(rumqttc::Packet::Publish(p))) => {
                    if p.topic == topic_in {
                        if let Err(e) = input_tx.send(p.payload.to_vec()) {
                            eprintln!("❌ Failed to forward input: {:?}", e);
                        }
                    } else if p.topic == topic_resize {
                        if
                            let Ok(resize_data) = serde_json::from_slice::<TerminalResize>(
                                &p.payload
                            )
                        {
                            println!(
                                "📏 Resize request: {}x{}",
                                resize_data.cols,
                                resize_data.rows
                            );
                            pty_master.master
                                .resize(PtySize {
                                    rows: resize_data.rows,
                                    cols: resize_data.cols,
                                    pixel_width: 0,
                                    pixel_height: 0,
                                })
                                .expect("Failed to resize pty");
                        }
                    }
                }
                Ok(rumqttc::Event::Incoming(rumqttc::Packet::ConnAck(_))) => {
                    println!("🟢 Connected to MQTT broker");
                }
                Ok(_) => {}
                Err(e) => {
                    eprintln!("❌ MQTT error: {:?}", e);
                    break;
                }
            }
        }

        publish_task.abort();
        status_task.abort();

        println!("🔄 Reconnecting in {} seconds...", reconnect_delay);
        tokio::time::sleep(Duration::from_secs(reconnect_delay)).await;
        reconnect_delay = std::cmp::min(reconnect_delay * 2, 30);
    }
}
