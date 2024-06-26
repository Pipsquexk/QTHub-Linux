// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use defines::{QTSInteraction, QTSOSCType};
use tauri::async_runtime::block_on;
use tauri::{AppHandle, Manager};
use rosc::{OscPacket, OscType};
use std::collections::HashMap;
use std::env;
use std::net::{SocketAddrV4, UdpSocket};
use std::str::FromStr;
use std::sync::Mutex;
use std::thread;

use dns_lookup::lookup_host;

use poem::{
    handler, listener::TcpListener, post,
    Route, Server, web::Json
};
use reqwest;


mod defines;


static QTSHOCK_SHK_STRENGTH: Mutex<i16> = Mutex::new(10);
static QTSHOCK_VIB_STRENGTH: Mutex<i16> = Mutex::new(80);
static QTSHOCK_IP: Mutex<String> = Mutex::new(String::new());

static VRC_OSC_THREAD: Mutex<bool> = Mutex::new(true);
static VRC_OSC_CANSHOCK: Mutex<bool> = Mutex::new(true);

static CS_CURRENT_DEATH_COUNT: Mutex<u16> = Mutex::new(0);



#[tauri::command]
async fn set_shock_strength(strength: i16) {
    *QTSHOCK_SHK_STRENGTH.lock().unwrap() = strength;
}

#[tauri::command]
async fn set_vibrate_strength(strength: i16) {
    *QTSHOCK_VIB_STRENGTH.lock().unwrap() = strength;
}





async fn death_check(data: Json<gsi_cs2::Body>) {
    let player = match data.player.as_ref() {
        Some(plyr) => {
            plyr
        },
        None => {
            return;
        }
    };
    let match_stats = match player.match_stats.as_ref() {
        Some(p_stats) => {
            p_stats
        },
        None => {
            return;
        }
    };
    let provider = match data.provider.as_ref() {
        Some(p_provider) => {
            p_provider
        },
        None => {
            return;
        }
    };

    if provider.steam_id != player.steam_id.clone().unwrap() {
        return;
    }

    if *CS_CURRENT_DEATH_COUNT.lock().unwrap() > match_stats.deaths {
        *CS_CURRENT_DEATH_COUNT.lock().unwrap() = 0;
    }

    if *CS_CURRENT_DEATH_COUNT.lock().unwrap() < match_stats.deaths {
        *CS_CURRENT_DEATH_COUNT.lock().unwrap() = match_stats.deaths;
        println!("Player died!");
        let mut map = HashMap::new();
        map.insert("_type", 1);
        map.insert("Strength", 2);

        let _ = trigger_qtshock(QTSInteraction::SHOCK);
    }
}









#[derive(Clone, serde::Serialize)]
struct Payload {
    message: String
}

fn trigger_qtshock(interaction: QTSInteraction) -> Result<(), String>{
    match interaction {
        QTSInteraction::SHOCK => {
            shock(QTSHOCK_SHK_STRENGTH.lock().unwrap().to_string().as_str());
            Ok(())
        },
        QTSInteraction::VIBRATE => {
            vibrate(QTSHOCK_VIB_STRENGTH.lock().unwrap().to_string().as_str());
            Ok(())
        },
        QTSInteraction::BEEP => {
            beep();
            Ok(())
        }
    }
}

#[tauri::command]
fn start_cs_listener() {
    
    let _new_thread = thread::spawn(|| {
        let _ = block_on(cs_thread());
    });
    beep();
}

#[handler]
async fn cs_update(data: Json<gsi_cs2::Body>) {
    death_check(data.clone()).await;
}

async fn cs_thread() -> Result<(), std::io::Error>{
    tracing_subscriber::fmt::init();

    let app = Route::new().at("/", post(cs_update));

    Server::new(TcpListener::bind("127.0.0.1:3000"))
        .run(app)
        .await
}


#[tauri::command]
fn start_vrc_osc(app: AppHandle, start: bool) {
    *VRC_OSC_THREAD.lock().unwrap() = start;
    if !start {
        return;
    }
    let _new_thread = thread::spawn(|| {
        vrc_osc_thread(app);
    });
    let _ = beep();
}



fn handle_packet(app: &AppHandle, packet: OscPacket) {
    let keep_thread: bool = *VRC_OSC_THREAD.lock().unwrap();
    if !keep_thread {
        return;
    }
    match packet {
        OscPacket::Message(msg) => {
            if !msg.addr.contains("QTS_") {
                return;
            }
            app.emit_all("vrc-osc-event", Payload { message: format!("VRC OSC msg | {}: {:?}", msg.addr, msg.args).into() }).unwrap();
            let addr_parts: Vec<&str> = msg.addr.split("_").collect();
            let qt_osc_type: QTSOSCType = match QTSOSCType::from_str(addr_parts[1]) {
                Ok(osc_type) => osc_type,
                _ => {
                    app.emit_all("vrc-osc-event", Payload { message: format!("Invalid QTShock OSC data received. Bad command type.").into() }).unwrap();
                    return;
                }
            };

            let qt_osc_interaction: QTSInteraction = match QTSInteraction::from_str(addr_parts[2]) {
                Ok(osc_interaction) => osc_interaction,
                _ => {
                    app.emit_all("vrc-osc-event", Payload { message: format!("Invalid QTShock OSC data received. Bad interaction type.").into() }).unwrap();
                    return;
                }
            };

            match qt_osc_type {
                QTSOSCType::PUSH => {
                    match msg.args[0] {
                        OscType::Float(f) => {
                            if f > 0.8f32 && *VRC_OSC_CANSHOCK.lock().unwrap() == true {
                                *VRC_OSC_CANSHOCK.lock().unwrap() = false;
                                match trigger_qtshock(qt_osc_interaction) {
                                    Ok(()) => {
                                        app.emit_all("vrc-osc-event", Payload { message: format!("Boop").into() }).unwrap();
                                    },
                                    _ => {
                                        app.emit_all("vrc-osc-event", Payload { message: format!("Something went wrong when triggering your QTShock.").into() }).unwrap();
                                    }

                                }
                            }
                            if f < 0.2f32 && *VRC_OSC_CANSHOCK.lock().unwrap() == false {
                                *VRC_OSC_CANSHOCK.lock().unwrap() = true;
                                app.emit_all("vrc-osc-event", Payload { message: format!("Unboop").into() }).unwrap();
                            }
                        },
                        _ => {
                            app.emit_all("vrc-osc-event", Payload { message: format!("Invalid QTShock OSC data received. Bad value type.").into() }).unwrap();
                        }
                    }
                },
                QTSOSCType::HIT => {
                    match msg.args[0] {
                        OscType::Bool(b) => {
                            app.emit_all("vrc-osc-event", Payload { message: format!("HIT!!!!!!!!!!!!!!!!!!!!").into() }).unwrap();
                            if b {
                                match trigger_qtshock(qt_osc_interaction) {
                                    Ok(()) => {
                                    },
                                    _ => {
                                        app.emit_all("vrc-osc-event", Payload { message: format!("Something went wrong when triggering your QTShock.").into() }).unwrap();
                                    }
    
                                }
                            }
                        },
                        _ => {
                            app.emit_all("vrc-osc-event", Payload { message: format!("Invalid QTShock OSC data received. Bad value type.").into() }).unwrap();
                        }
                    }
                }
            }
        }
        OscPacket::Bundle(bundle) => {
            app.emit_all("vrc-osc-event", Payload { message: format!("VRC OSC bundle | {:?}", bundle).into() }).unwrap();

        }
    }
}

fn vrc_osc_thread(app: AppHandle) {
    let addr = match SocketAddrV4::from_str("127.0.0.1:9001") {
        Ok(addr) => {
            println!("Got proper address!");
            addr
        },
        Err(_) => {
            println!("FAILED TO GET ADDRESS!");
            return;
        }
    };
    let sock = UdpSocket::bind(addr).unwrap();
    app.emit_all("vrc-osc-event", Payload { message: format!("Listening to {}", addr).into() }).unwrap();
    println!("Listening to {}", addr);

    let mut buf = [0u8; rosc::decoder::MTU];

    loop {
        let keep_thread: bool = *VRC_OSC_THREAD.lock().unwrap();
        if !keep_thread {
            break;
        }
        match sock.recv_from(&mut buf) {
            Ok((size, _addr)) => {
                let (_, packet) = rosc::decoder::decode_udp(&buf[..size]).unwrap();
                handle_packet(&app, packet);
            }
            Err(e) => {
                println!("Error receiving from socket: {}", e);
                break;
            }
        }
    }
    println!("VRC OSC Socket closed!");
    app.emit_all("vrc-osc-event", Payload { message: "VRC OSC Socket closed".into() }).unwrap();
}


#[tauri::command]
fn load_local_ip() -> String {
    let hostname = "qtshock.local";
    match lookup_host(hostname) {
        Ok(ips) => {
            *QTSHOCK_IP.lock().unwrap() = ips[0].to_string();
            ips[0].to_string()
        },
        Err(err) => {
            format!("Failed to find a QTShock on the network! Error: {}", err.to_string()).to_string()
        }
    }
    
}

#[tauri::command]
fn shock(strength: &str) -> String {
    match strength.to_string().parse::<i16>() {
        Ok(i) => {
            if i < 1 || i > 99 {
                return "".to_string();
            }
            let params = [("strength", strength)];
            let client = reqwest::blocking::Client::new();
            let _res = client.post(format!("http://{}/shock", QTSHOCK_IP.lock().unwrap()))
            .form(&params)
            .send()
            .unwrap();
            format!("Shock was called with: {}", strength)
        },
        _ => {
            "BAD SHOCK CALL".to_string()
        }
    }
    
}

#[tauri::command]
fn vibrate(strength: &str) -> String {
    match strength.to_string().parse::<i16>() {
        Ok(i) => {
            if i < 1 || i > 99 {
                return "".to_string();
            }
            let params = [("strength", strength)];
            let client = reqwest::blocking::Client::new();
            let _res = client.post(format!("http://{}/vibrate", QTSHOCK_IP.lock().unwrap()))
            .form(&params)
            .send()
            .unwrap();
            format!("Vibrate was called with: {}", strength)
        },
        _ => {
            "BAD VIBRATE CALL".to_string()
        }
    }
}

#[tauri::command]
fn beep() -> String {
    let client = reqwest::blocking::Client::new();
    let _res = client.post(format!("http://{}/beep", QTSHOCK_IP.lock().unwrap()))
    .send()
    .unwrap();
    format!("Beep was called")
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![load_local_ip, set_shock_strength, set_vibrate_strength, start_cs_listener, start_vrc_osc, shock, vibrate, beep])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
