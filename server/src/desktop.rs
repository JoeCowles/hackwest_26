use eframe::egui::{self, Color32, RichText};
use orchard_server::store::AppState;
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{Arc, RwLock, mpsc},
    time::Duration,
};

pub fn run(
    state: AppState,
    runtime: tokio::runtime::Handle,
    status: Arc<RwLock<String>>,
    url: String,
    admin_token: String,
    directory: PathBuf,
) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Orchard Server")
            .with_inner_size([760.0, 570.0])
            .with_min_inner_size([620.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Orchard Server",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::light());
            let mut style = (*cc.egui_ctx.style()).clone();
            style.spacing.item_spacing = egui::vec2(12.0, 10.0);
            style.visuals.panel_fill = Color32::from_rgb(242, 242, 243);
            style.visuals.selection.bg_fill = Color32::from_rgb(89, 128, 166);
            cc.egui_ctx.set_style(style);
            Ok(Box::new(Console {
                state,
                runtime,
                status,
                url,
                admin_token,
                directory,
                pending: None,
                enrollment: None,
                message: String::new(),
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!(e.to_string()))
}

struct Console {
    state: AppState,
    runtime: tokio::runtime::Handle,
    status: Arc<RwLock<String>>,
    url: String,
    admin_token: String,
    directory: PathBuf,
    pending: Option<mpsc::Receiver<Result<Value, String>>>,
    enrollment: Option<Value>,
    message: String,
}

impl eframe::App for Console {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_secs(1));
        if let Some(receiver) = &self.pending {
            if let Ok(result) = receiver.try_recv() {
                match result {
                    Ok(value) => {
                        self.enrollment = Some(value);
                        self.message =
                            "Single-use token ready. It expires after 10 minutes.".into();
                    }
                    Err(error) => self.message = error,
                }
                self.pending = None;
            }
        }
        egui::CentralPanel::default().show(ctx,|ui|{
            ui.add_space(14.0);
            ui.label(RichText::new("ORCHARD / SERVER").size(28.0).strong());
            ui.label("Storage telemetry control center");
            ui.separator();
            ui.label(RichText::new(self.status.read().map(|s|s.clone()).unwrap_or_else(|_|"Status unavailable".into())).color(Color32::from_rgb(60,104,143)));
            ui.horizontal(|ui|{ui.monospace(&self.url);if ui.button("Copy API address").clicked(){ctx.copy_text(self.url.clone());}});
            ui.add_space(12.0);
            let stats=self.state.statistics.read().map(|s|s.clone()).unwrap_or_default();
            ui.columns(3,|columns|{
                for (column,(label,value)) in columns.iter_mut().zip([("ENROLLED NODES",stats.nodes),("STORED SAMPLES",stats.samples),("ACCEPTED BATCHES",stats.batches)]){
                    column.label(RichText::new(label).small());column.label(RichText::new(value.to_string()).size(36.0).strong());
                }
            });
            ui.separator();
            ui.heading("Connect a node");
            ui.label("Create one enrollment token per node, then exchange it using POST /api/v1/nodes/enroll.");
            if ui.add_enabled(self.pending.is_none(),egui::Button::new("Create enrollment token")).clicked(){
                let (sender,receiver)=mpsc::channel();self.pending=Some(receiver);
                let state=self.state.clone();
                self.runtime.spawn(async move{let result=state.local_enrollment_token().await.map_err(|e|e.message);let _=sender.send(result);});
            }
            if let Some(token)=&self.enrollment{
                ui.horizontal(|ui|{ui.label("Enrollment token is hidden.");if ui.button("Copy enrollment token").clicked(){ctx.copy_text(token["enrollment_token"].as_str().unwrap_or_default().to_owned());}});
                ui.small(format!("Expires {}",token["expires_at"].as_str().unwrap_or_default()));
            }
            ui.label(&self.message);
            ui.add_space(8.0);
            ui.collapsing("Administration and storage",|ui|{
                ui.label("The admin credential can create enrollment tokens and revoke node credentials.");
                if ui.button("Copy admin credential").clicked(){ctx.copy_text(self.admin_token.clone());}
                ui.small(format!("Data: {}",self.directory.display()));
                if let Some(last)=&stats.last_maintenance{ui.small(format!("Last retention / rollup pass: {last}"));}
                if let Some(error)=&stats.error{ui.colored_label(Color32::from_rgb(166,89,100),error);}
            });
            ui.separator();
            ui.label("Heartbeat: every 5 seconds. Closing this application stops the server.");
            ui.small("Web and monitoring read endpoints are planned in Server Spec section 14.");
            ui.hyperlink_to("Open the shared Server Spec","https://docs.google.com/document/d/1JnsZHlYPXQ1IRsMSEHeboICzqqviKJylI7gRsuUK13E/edit?tab=t.bclfm0yxwd6r");
        });
    }
}
