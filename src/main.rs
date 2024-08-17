use worker::*;
use chrono::{DateTime, Duration, Utc};
use reqwest::Client;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

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

#[event(fetch)]
async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let router = Router::new();

    router
        .get("/status", |_, ctx| async move {
            let kv = ctx.kv("STATUS_STORE")?;
            let status: Option<Status> = kv.get("current_status").json().await?;

            match status {
                Some(s) => Response::from_json(&s),
                None => Response::error("No status available", 404),
            }
        })
        .run(req, env)
        .await
}

#[event(scheduled)]
pub async fn cron(event: ScheduledEvent, env: Env, _ctx: Context) {
    let client = Client::new();
    match scrape_status(&client).await {
        Ok((schwebebahn, elevators)) => {
            let status = Status {
                schwebebahn,
                elevators,
                last_updated: Some(Utc::now()),
            };

            let kv = env.kv("STATUS_STORE").unwrap();
            if let Err(e) = kv.put("current_status", serde_json::to_string(&status).unwrap()).await {
                console_log!("Error saving status to KV: {:?}", e);
            }
        },
        Err(e) => console_log!("Error scraping status: {}", e),
    }
}
