use actix_web::{web, App, HttpResponse, HttpServer};
use chrono::{DateTime, Duration, Utc};
use reqwest::Client;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tokio::time::interval;

#[derive(Clone, Serialize, Deserialize, Debug)]
struct ElevatorStatus {
    station: String,
    event: String,
    start_time: String,
    end_time: String,
    location: String,
    info: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
struct Status {
    schwebebahn: Vec<String>,
    elevators: Vec<ElevatorStatus>,
    #[serde(with = "chrono::serde::ts_seconds_option")]
    last_updated: Option<DateTime<Utc>>,
}

struct AppState {
    last_api_request: Mutex<Option<DateTime<Utc>>>,
    status: Mutex<Status>,
}

async fn scrape_status(client: &Client) -> Result<(Vec<String>, Vec<ElevatorStatus>), Box<dyn std::error::Error>> {
    let url = "https://www.wsw-online.de/mobilitaet/fahrplan/fahrtauskunft/verkehrsinformationen/";
    let response = client.get(url).send().await?.text().await?;
    let document = Html::parse_document(&response);

    let row_selector = Selector::parse("tr.traffic-information-infos").unwrap();
    let mut schwebebahn_status = Vec::new();
    let mut elevator_status = Vec::new();

    for row in document.select(&row_selector) {
        let transportation = row.value().attr("data-transportation").unwrap_or("");
        
        match transportation {
            "elevator" => {
                let status = parse_elevator_status(&row, &document);
                elevator_status.push(status);
            },
            "subway" => {
                let info = parse_schwebebahn_status(&row);
                schwebebahn_status.push(info);
            },
            _ => continue,
        }
    }

    if schwebebahn_status.is_empty() {
        schwebebahn_status.push("Keine aktuellen Störungen".to_string());
    }

    if elevator_status.is_empty() {
        elevator_status.push(ElevatorStatus {
            station: String::new(),
            event: "Keine Störungen".to_string(),
            start_time: String::new(),
            end_time: String::new(),
            location: String::new(),
            info: "Alle Aufzüge sind in Betrieb".to_string(),
        });
    }

    Ok((schwebebahn_status, elevator_status))
}

fn parse_elevator_status(row: &scraper::element_ref::ElementRef, document: &Html) -> ElevatorStatus {
    let station = row.select(&Selector::parse("td.cell-line span.fw-bold").unwrap()).next()
        .and_then(|el| el.text().next())
        .unwrap_or("").trim().to_string();

    let event = row.select(&Selector::parse("td.cell-event span.flag").unwrap()).next()
        .and_then(|el| el.text().next())
        .unwrap_or("").trim().to_string();

    let period = row.select(&Selector::parse("td.cell-period").unwrap()).next()
        .map(|el| el.text().collect::<String>())
        .unwrap_or_default().trim().to_string();

    let location = row.select(&Selector::parse("td.cell-location").unwrap()).next()
        .and_then(|el| el.text().next())
        .unwrap_or("").trim().to_string();

    let info_selector = Selector::parse(&format!("#{} p:last-child", row.value().attr("id").unwrap_or(""))).unwrap();
    let info = document.select(&info_selector).next()
        .and_then(|el| el.text().next())
        .unwrap_or("").trim().to_string();

    let (start_time, end_time) = parse_period(&period);

    ElevatorStatus {
        station,
        event,
        start_time,
        end_time,
        location,
        info,
    }
}

fn parse_schwebebahn_status(row: &scraper::element_ref::ElementRef) -> String {
    format!("{}: {}", 
        row.select(&Selector::parse("td.cell-event span.flag").unwrap()).next()
            .and_then(|el| el.text().next())
            .unwrap_or("").trim(),
        row.select(&Selector::parse("td.cell-location").unwrap()).next()
            .and_then(|el| el.text().next())
            .unwrap_or("").trim()
    )
}

fn parse_period(period: &str) -> (String, String) {
    let parts: Vec<&str> = period.split("bis").collect();
    let start = parts.get(0).map_or("", |s| s.trim());
    let end = parts.get(1).map_or("", |s| s.trim());
    (start.to_string(), end.to_string())
}

async fn status(data: web::Data<Arc<AppState>>) -> HttpResponse {
    let mut last_request = data.last_api_request.lock().unwrap();
    *last_request = Some(Utc::now());
    
    let status = data.status.lock().unwrap().clone();
    HttpResponse::Ok().json(status)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let state = Arc::new(AppState {
        last_api_request: Mutex::new(None),
        status: Mutex::new(Status {
            schwebebahn: Vec::new(),
            elevators: Vec::new(),
            last_updated: None,
        }),
    });

    let state_clone = Arc::clone(&state);
    tokio::spawn(async move {
        let mut interval = interval(Duration::minutes(15).to_std().unwrap());
        let client = Client::new();

        loop {
            if should_check(&state_clone) {
                match scrape_status(&client).await {
                    Ok((schwebebahn, elevators)) => {
                        let mut app_status = state_clone.status.lock().unwrap();
                        app_status.schwebebahn = schwebebahn;
                        app_status.elevators = elevators;
                        app_status.last_updated = Some(Utc::now());
                        println!("Status updated: {:?}", app_status);
                    },
                    Err(e) => eprintln!("Error scraping status: {}", e),
                }
            }
            interval.tick().await;
        }
    });

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(Arc::clone(&state)))
            .route("/status", web::get().to(status))
    })
    .bind("0.0.0.0:8070")?
    .run()
    .await
}

fn should_check(state: &Arc<AppState>) -> bool {
    let last_request = state.last_api_request.lock().unwrap();
    match *last_request {
        Some(time) => Utc::now() - time < Duration::minutes(20),
        None => false,
    }
}