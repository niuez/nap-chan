mod lib;
use serde_json::to_string;
use serenity::model::id::GuildId;
use serenity::model::prelude::VoiceState;
use serenity::prelude::TypeMapKey;
use songbird::{Event, EventContext, SerenityInit, TrackEvent};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

use dotenv::dotenv;
use lib::voice::*;
use serenity::client::Context;
use serenity::{
    async_trait,
    client::{Client, EventHandler},
    framework::{
        standard::{
            macros::{command, group},
            Args, CommandResult,
        },
        StandardFramework,
    },
    model::{channel::Message, gateway::Ready},
};

const DICT_PATH: &str = "read_dict.json";

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        tracing::info!("{} is connected!", ready.user.name);
    }
    async fn voice_state_update(
        &self,
        _ctx: Context,
        _: Option<GuildId>,
        _old: Option<VoiceState>,
        _new: VoiceState,
    ) {
        tracing::info!("{:?}\n{:?}", _old, _new);
        tracing::info!("{} is connected!", _new.member.unwrap().user.name);
    }
    async fn message(&self, ctx: Context, msg: Message) {
        play_voice(&ctx, msg).await;
    }
}

#[group]
#[commands(join, leave, mute, unmute, play, add)]
struct General;

struct TrackEndNotifier;

#[async_trait]
impl songbird::EventHandler for TrackEndNotifier {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        if let EventContext::Track(track_list) = ctx {
            for (_, handle) in track_list.iter() {
                std::fs::remove_file(Path::new(handle.metadata().source_url.as_ref().unwrap()))
                    .unwrap();
            }
        }
        None
    }
}

struct DictHandler;

impl TypeMapKey for DictHandler {
    type Value = Arc<Mutex<HashMap<String, String>>>;
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();
    dotenv().ok();
    let token = std::env::var("VOICEVOX_TOKEN").expect("environment variable not found");
    let dict_file = std::fs::File::open(DICT_PATH).unwrap();
    let reader = std::io::BufReader::new(dict_file);
    let dict: HashMap<String, String> = serde_json::from_reader(reader).unwrap();
    let framework = StandardFramework::new()
        .configure(|c| c.prefix(">"))
        .group(&GENERAL_GROUP);
    let mut client = Client::builder(&token)
        .event_handler(Handler)
        .framework(framework)
        .register_songbird()
        .await
        .expect("Err creating client");
    {
        let mut data = client.data.write().await;
        data.insert::<DictHandler>(Arc::new(Mutex::new(dict)));
    }
    tokio::spawn(async move {
        let _ = client
            .start()
            .await
            .map_err(|why| tracing::info!("Client ended: {:?}", why));
    });

    tokio::signal::ctrl_c().await.unwrap();
    tracing::info!("Ctrl-C received, shutting down...");
}

#[command]
#[only_in(guilds)]
async fn join(ctx: &Context, msg: &Message) -> CommandResult {
    let guild = msg.guild(&ctx.cache).await.unwrap();
    let guild_id = guild.id;
    let channel_id = guild
        .voice_states
        .get(&msg.author.id)
        .and_then(|voice_state| voice_state.channel_id);

    let connect_to = match channel_id {
        Some(channel) => channel,
        None => {
            msg.reply(ctx, "Not in a voice channel").await?;
            return Ok(());
        }
    };

    let manager = songbird::get(ctx)
        .await
        .expect("Songbird Voice client placed in at initialisation.")
        .clone();

    let (handle_lock, _) = manager.join(guild_id, connect_to).await;
    let mut handle = handle_lock.lock().await;
    handle.deafen(true).await.unwrap();
    handle.add_global_event(Event::Track(TrackEvent::End), TrackEndNotifier);
    Ok(())
}

#[command]
#[only_in(guilds)]
async fn leave(ctx: &Context, msg: &Message) -> CommandResult {
    let guild = msg.guild(&ctx.cache).await.unwrap();
    let guild_id = guild.id;
    let manager = songbird::get(ctx)
        .await
        .expect("Songbird Voice client placed in at initialisation.")
        .clone();
    let has_handler = manager.get(guild_id).is_some();

    if has_handler {
        if let Err(e) = manager.remove(guild_id).await {
            msg.channel_id
                .say(&ctx.http, format!("Failed: {:?}", e))
                .await?;
        }

        msg.channel_id.say(&ctx.http, "Left voice channel").await?;
    } else {
        msg.reply(ctx, "Not in a voice channel").await?;
    }
    Ok(())
}

#[command]
#[only_in(guilds)]
async fn mute(ctx: &Context, msg: &Message) -> CommandResult {
    let guild = msg.guild(&ctx.cache).await.unwrap();
    let guild_id = guild.id;

    let manager = songbird::get(ctx)
        .await
        .expect("Songbird Voice client placed in at initialisation.")
        .clone();

    let handler_lock = match manager.get(guild_id) {
        Some(handler) => handler,
        None => {
            msg.reply(ctx, "Not in a voice channel").await?;
            return Ok(());
        }
    };

    let mut handler = handler_lock.lock().await;

    let content = if handler.is_mute() {
        "Already muted".to_string()
    } else {
        if let Err(e) = handler.mute(true).await {
            format!("Failed: {:?}", e)
        } else {
            "Now muted".to_string()
        }
    };
    msg.channel_id.say(&ctx.http, content).await?;
    Ok(())
}

#[command]
#[only_in(guilds)]
async fn unmute(ctx: &Context, msg: &Message) -> CommandResult {
    let guild = msg.guild(&ctx.cache).await.unwrap();
    let guild_id = guild.id;

    let manager = songbird::get(ctx)
        .await
        .expect("Songbird Voice client placed in at initialisation.")
        .clone();

    let content = if let Some(handler_lock) = manager.get(guild_id) {
        let mut handler = handler_lock.lock().await;
        if let Err(e) = handler.mute(false).await {
            format!("Failed: {:?}", e)
        } else {
            "Unmuted".to_string()
        }
    } else {
        "Not in a voice channel to unmute in".to_string()
    };
    msg.channel_id.say(&ctx.http, content).await?;
    Ok(())
}



#[command]
#[only_in(guilds)]
async fn play(ctx: &Context, msg: &Message, mut args: Args) -> CommandResult {
    let url = match args.single::<String>() {
        Ok(url) => url,
        Err(_) => {
            msg.channel_id
                .say(&ctx.http, "Must provide a URL to a video or audio")
                .await?;
            return Ok(());
        }
    };

    if !url.starts_with("http") {
        msg.channel_id
            .say(&ctx.http, "Must provide a valid URL")
            .await?;
        return Ok(());
    }

    let guild = msg.guild(&ctx.cache).await.unwrap();
    let guild_id = guild.id;

    let manager = songbird::get(ctx)
        .await
        .expect("Songbird Voice client placed in at initialisation.")
        .clone();

    let content = if let Some(handler_lock) = manager.get(guild_id) {
        let mut handler = handler_lock.lock().await;

        match songbird::ytdl(&url).await {
            Ok(source) => {
                handler.play_source(source);
                "Playing song"
            }
            Err(why) => {
                tracing::error!("Err starting source: {:?}", why);
                "Error sourcing ffmpeg"
            }
        }
    } else {
        "Not in a voice channel to play in"
    };
    msg.channel_id.say(&ctx.http, content).await?;
    Ok(())
}

#[command]
#[only_in(guild)]
#[num_args(2)]
async fn add(ctx: &Context, _msg: &Message, mut args: Args) -> CommandResult {
    let before: String = args.single().unwrap();
    let after: String = args.single().unwrap();
    dbg!(&before, &after);
    let dict_lock = {
        let data_read = ctx.data.read().await;
        data_read.get::<DictHandler>().unwrap().clone()
    };
    let mut dict = dict_lock.lock().await;
    dict.insert(before, after);
    let dict = dict.clone();
    let dict_json = to_string(&dict).unwrap();
    let mut dict_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .truncate(true)
        .open(DICT_PATH)
        .unwrap();
    dict_file.write_all(dict_json.as_bytes()).unwrap();
    dict_file.flush().unwrap();

    Ok(())
}

#[command]
#[only_in(guild)]
#[num_args(1)]
async fn rem(ctx: &Context, _: &Message, mut args: Args) -> CommandResult {
    let before: String = args.single().unwrap();
    let dict_lock = {
        let data_read = ctx.data.read().await;
        data_read.get::<DictHandler>().unwrap().clone()
    };
    let mut dict = dict_lock.lock().await;
    if dict.contains_key(&before) {
        dict.remove(&before);
    }
    let dict = dict.clone();
    let dict_json = to_string(&dict).unwrap();
    let mut dict_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .truncate(true)
        .open("read_dict.json")
        .unwrap();
    dict_file.write_all(dict_json.as_bytes()).unwrap();
    dict_file.flush().unwrap();
    Ok(())
}
