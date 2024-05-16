use std::{collections::HashMap, fmt::Debug, num::ParseIntError, sync::{atomic::AtomicBool, Arc}};

use egui::Color32;
use image::DynamicImage;
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, sync::RwLock};

use crate::{load_misskey::{RawFile, RawInstance, RawNote, RawUser}, ConfigFile};

const DUMMY_PNG:&'static str="local://dummy.png";
pub const DEFAULT_ANIMATION:bool=true;
pub(crate) fn cache_dir()->String{
	std::env::var("YAC_CACHE_PATH").unwrap_or_else(|_|"cache".to_owned())
}
#[derive(Debug)]
pub struct Note{
	pub id:String,
	pub user:Arc<UserProfile>,
	pub quote:Option<Arc<Note>>,
	pub text:MFMString,
	pub visibility:Visibility,
	pub reactions: Reactions,
	pub files:Vec<NoteFile>,
	pub cw:Option<MFMString>,
	pub created_at: chrono::prelude::DateTime<chrono::prelude::Utc>,
}
impl PartialEq for Note{
	fn eq(&self, other: &Self) -> bool {
		self.id==other.id&&
		self.reactions.all==other.reactions.all&&
		self.quote.is_some()==other.quote.is_some()&&
		self.created_at==other.created_at
	}
}
impl Note{
	pub fn created_at_label(&self)->String{
		let secs_ago=chrono::Utc::now().timestamp()-self.created_at.timestamp();
		if secs_ago>12*30*24*60*60{
			format!("{}年前",secs_ago/(12*30*24*60*60))
		}else if secs_ago>30*24*60*60{
			format!("{}ヶ月前",secs_ago/(30*24*60*60))
		}else if secs_ago>7*24*60*60{
			format!("{}週間前",secs_ago/(7*24*60*60))
		}else if secs_ago>24*60*60{
			format!("{}日前",secs_ago/(24*60*60))
		}else if secs_ago>60*60{
			format!("{}時間前",secs_ago/(60*60))
		}else if secs_ago>60{
			format!("{}分前",secs_ago/60)
		}else{
			format!("{}秒前",secs_ago)
		}
	}
	pub async fn system_message(text:impl Into<String>,name:impl Into<String>)->Self{
		let emoji_cache=EmojiCache::new("","localhost",Arc::new(HashMap::new()));
		let instance:Option<Arc<FediverseInstance>>=None;
		Self{
			created_at:chrono::Utc::now(),
			id:uuid::Uuid::new_v4().to_string(),
			user: Arc::new(UserProfile{
				id: "system".to_owned(),
				username: "system".to_owned(),
				display_name: MFMString::new(name.into(),None,instance.clone(),&emoji_cache).await,
				instance: None,
				icon: DUMMY_PNG.to_owned().into(),
			}),
			quote: None,
			text: MFMString::new(text.into(),None,instance,&emoji_cache).await,
			visibility: Visibility::Public,
			reactions: Reactions{
				emojis: vec![],
				all:0,
			},
			files: vec![],
			cw:None,
		}
	}
}
#[derive(Clone,Debug)]
pub struct NoteFile{
	pub(crate) img:Option<Arc<UrlImage>>,
	pub original_url:Option<String>,
	pub blurhash: Option<Arc<UrlImage>>,
	pub is_sensitive:bool,
	pub show_sensitive:Arc<AtomicBool>,
}
impl From<&RawFile> for NoteFile{
	fn from(value: &RawFile) -> Self {
		let img=if value.mime_type.as_ref().map(|s|s.starts_with("image/")).unwrap_or(false){
			if let Some(url)=value.thumbnail_url.as_ref(){
				Some(Arc::new(UrlImage::from(url.to_owned())))
			}else{
				None
			}
		}else{
			None
		};
		if img.is_none(){
			println!("{:?} not image",&value.mime_type);
		}else{
			println!("{:?} {}",&value.mime_type,img.as_ref().unwrap().url);
		}
		let blurhash=match (&value.blurhash,value.properties.as_ref().map(|v|(v.width,v.height))){
			(Some(blurhash),Some((Some(width),Some(height))))=>{
				let width=1.max(width/10);
				let height=1.max(height/10);
				blurhash_wasm::decode(blurhash,width as usize,height as usize).map(|res|{
					image::RgbaImage::from_raw(width,height,res).map(|img|{
						image::DynamicImage::ImageRgba8(img)
					})
				}).unwrap_or_default().map(|img|{
					Arc::new(UrlImage::new(format!("blurhash://{}",blurhash),TextureState::from(vec![(0,img)])))
				})
			},
			_=>None
		};
		Self{
			img,
			blurhash,
			is_sensitive:value.is_sensitive,
			original_url:value.url.clone(),
			show_sensitive:Arc::new(AtomicBool::new(false)),
		}
	}
}
impl NoteFile{
	pub fn image(&self,animate_frame:u64)->Option<egui::Image<'static>>{
		let img=self.img.as_ref()?;
		img.get(animate_frame)
	}
	pub fn is_image(&self)->bool{
		self.img.is_some()
	}
}
#[derive(PartialEq,Eq,Clone, Copy,Debug)]
pub enum Visibility {
    Public,
    Home,
    Followers,
    Specified,
}
impl From<&str> for Visibility{
	fn from(value: &str) -> Self {
		match value{
			"public"=>Self::Public,
			"home"=>Self::Home,
			"followers"=>Self::Followers,
			_=>Self::Specified
		}
	}
}
#[derive(Debug)]
pub struct UserProfile{
	id:String,//9fftrmo3sw
	pub username:String,//kozakura
	pub display_name:MFMString,//狐桜
	pub instance:Option<Arc<FediverseInstance>>,//misskey.kzkr.xyz
	pub icon:UrlImage,//https://misskey.kzkr.xyz/avatar/@kozakura@misskey.kzkr.xyz
}
impl UserProfile{
	pub async fn load(
		user:&RawUser,
		cache:&mut HashMap<String,Arc<FediverseInstance>>,
		emoji_cache:&EmojiCache,
	) -> Self {
		let f=match (&user.host,&user.instance) {
			(Some(host),Some(instance))=>{
				Some(match cache.get(host){
					Some(cache_hit)=>cache_hit.clone(),
					None=>{
						let mut f=FediverseInstance::new(instance,&emoji_cache.media_proxy);
						f.host=user.host.clone().unwrap();
						let f=Arc::new(f);
						cache.insert(host.clone(),f.clone());
						f
					}
				})
			},
			_=>None
		};
		let icon_url=user.avatar_url.as_ref().map(|u|u.to_string()).unwrap_or_else(||
			format!("{}/avatar/@{}{}",emoji_cache.local_instance.as_str(),user.username,user.host.as_ref().map(|s|format!("@{}",s)).unwrap_or_default())
		);
		let display_name=MFMString::new(user.name.as_ref().unwrap_or_else(||&user.username).to_owned(),user.emojis.as_ref(),f.as_ref(),&emoji_cache).await;
		Self{
			instance:f,
			id: user.id.to_string(),
			display_name,
			username: user.username.clone(),
			icon: icon_url.into(),
		}
	}
}
#[derive(Clone,Debug)]
pub struct EmojiCache{
	media_proxy:String,
	local_instance:String,
	map:Arc<RwLock<HashMap<String,Arc<UrlImage>>>>,
	local_emojis:Arc<HashMap<String,String>>,
}
impl EmojiCache{
	pub fn new(media_proxy:impl Into<String>,local_instance:impl Into<String>,local_emojis:Arc<HashMap<String,String>>)->Self{
		Self{
			media_proxy:media_proxy.into(),
			local_instance:local_instance.into(),
			map:Arc::new(RwLock::new(HashMap::new())),
			local_emojis,
		}
	}
	pub async fn load(&self,unique_emoji_id:String,url:&str)->Emoji{
		match self.map.read().await.get(&unique_emoji_id){
			Some(hit) => return Emoji{
				id: unique_emoji_id,
				img: hit.clone(),
			},
			None => {},
		}
		println!("load emoji {}",unique_emoji_id);
		let remote_url=urlencoding::encode(url);
		let local_url=format!("{}/emoji.webp?url={}&emoji=1",self.media_proxy,remote_url);
		let img:Arc<UrlImage>=Arc::new(local_url.into());
		self.map.write().await.insert(unique_emoji_id.clone(),img.clone());
		Emoji{
			id:unique_emoji_id,
			img,
		}
	}
	pub async fn trim(&self,rc:usize)->usize{
		let mut lock=self.map.write().await;
		let mut remove_targets=vec![];
		for (k,v) in lock.iter(){
			if v.loaded(){
				let count=Arc::strong_count(v);
				if count<rc{
					remove_targets.push(k.clone());
				}
			}
		}
		for r in &remove_targets{
			lock.remove(r);
		}
		remove_targets.len()
	}
}
#[derive(Debug)]
pub struct Reactions{
	pub emojis:Vec<(Emoji,u64)>,
	pub all:u128,
}
impl Reactions{
	pub fn emojis(&self)->impl Iterator<Item=&Arc<UrlImage>>{
		self.emojis.iter().map(|(emoji,_)|&emoji.img)
	}
}
impl Reactions{
	pub async fn load(
		note: &RawNote,
		emoji_cache:&EmojiCache,
	) -> Self {
		let mut emojis=vec![];
		let mut all_count=0;
		for (reaction,count) in &note.reactions{
			if reaction.ends_with("@.:"){//isLocalEmoji
				let id=reaction[1..reaction.len()-3].to_string();
				let url=emoji_cache.local_emojis.get(&id);
				if let Some(url)=url{
					let emoji=emoji_cache.load(id,url.as_str()).await;
					all_count+=*count as u128;
					emojis.push((emoji,*count));
				}else{
					println!("ローカル絵文字が見つからない?{}",id);
				}
			}else if reaction.contains("@"){
				//リモート絵文字
				let id=reaction[1..reaction.len()-1].to_string();
				let url=note.reaction_emojis.get(&id);
				if let Some(url)=url{
					let emoji=emoji_cache.load(id,url.as_str()).await;
					all_count+=*count as u128;
					emojis.push((emoji,*count));
				}else{
					println!("リモート絵文字が見つからない?{}",id);
				}
			}else{
				//おそらくUnicode絵文字
				//let id=hex::encode(reaction.0.as_bytes());
				if let Some((id,url))=unicode_to_emoji(&reaction,&emoji_cache.local_instance){
					let emoji=emoji_cache.load(id,url.as_str()).await;
					all_count+=*count as u128;
					emojis.push((emoji,*count));
				}else{
					println!("Unicode絵文字が見つからない?{}",reaction);
				}
			}
		}
		emojis.sort_by(|(_,a),(_,b)|b.cmp(a));
		Self {
			emojis,
			all:all_count,
		}
	}
}
fn unicode_to_emoji(unicode:&str,local_instance:&str)->Option<(String,String)>{
	let c=unicode.chars().next()?;
	let id=hex::encode((c as u32).to_be_bytes());
	let mut offset=0;
	for c in id.chars(){
		if c=='0'{
			offset+=1;
		}else{
			break;
		}
	}
	let id=id[offset..].to_owned();
	let url=format!("{}/twemoji/{}.svg",local_instance,&id);
	Some((id,url))
}
#[derive(Debug)]
pub struct MFMString{
	raw:String,
	render:Vec<MFMElement>,
}
#[derive(Debug)]
enum MFMElement{
	Text(String),
	Emoji(Emoji),
	Scale(f32,Box<MFMElement>),
}
struct MFMRenderContext{
	scale:f32,
}
impl MFMRenderContext{
	fn new()->Self{
		Self{
			scale:1f32,
		}
	}
}
impl MFMElement{
	pub fn render(&self,ui:&mut egui::Ui,strong:bool,dummy:&UrlImage,ctx:&mut MFMRenderContext,animate_frame:u64){
		use egui::Widget;
		match self{
			MFMElement::Text(s)=>{
				let text=egui::RichText::from(s);
				let text=if strong{
					text.strong()
				}else{
					text
				};
				let text=text.size(12f32*ctx.scale);
				egui::Label::new(text).ui(ui);
			},
			MFMElement::Emoji(emoji)=>{
				let img=emoji.img.get(animate_frame).unwrap_or_else(||dummy.get(animate_frame).unwrap());
				let img=img.max_size([f32::MAX,20f32*ctx.scale].into());
				img.ui(ui).on_hover_text(emoji.id.as_str());
			},
			MFMElement::Scale(s,e)=>{
				ctx.scale*=s;
				e.render(ui,strong,dummy,ctx,animate_frame);
			},
		}
	}
}
#[derive(Debug)]
pub struct Emoji{
	id:String,
	img:Arc<UrlImage>,
}
impl Emoji{
	pub fn image(&self,animate_frame:u64)->Option<egui::Image<'static>>{
		self.img.get(animate_frame)
	}
	pub fn id(&self)->&str{
		&self.id
	}
	pub fn url_image(&self)->&Arc<UrlImage>{
		&self.img
	}
}
impl MFMString{
	pub fn is_empty(&self)->bool{
		self.raw.is_empty()||self.render.is_empty()
	}
	pub async fn new_opt(
		raw:Option<String>,
		known_emojis: Option<&HashMap<String,String>>,
		instance:Option<impl AsRef<FediverseInstance>>,
		emoji_cache:&EmojiCache,
	)->Option<Self>{
		if raw.is_some(){
			Some(Self::new(raw.unwrap(),known_emojis,instance,emoji_cache).await)
		}else{
			None
		}
	}
	pub async fn new(
		raw:String,
		known_emojis: Option<&HashMap<String,String>>,
		instance:Option<impl AsRef<FediverseInstance>>,
		emoji_cache:&EmojiCache,
	)->Self{
		let known_emojis=if instance.is_none(){
			Some(emoji_cache.local_emojis.as_ref())
		}else{
			known_emojis
		};
		let instance_str=instance.map(|s|format!("@{}",s.as_ref().host)).unwrap_or_default();
		//println!("raw={}",raw);
		let mut render=vec![];
		let mut emoji_indexs=vec![];
		if let Some(emojis)=known_emojis{
			for (k,url) in emojis{
				let mut s=raw.as_str();
				let mut offset=0;
				loop{
					let k=format!(":{}:",k);
					if let Some(idx)=s.find(&k){
						let len=k.len();
						emoji_indexs.push((idx+offset,k,len,url.to_string()));
						offset=offset+idx+len;
						s=&raw[offset..];
					}else{
						break;
					}
				}
			}
		}
		let emoji_match=regex::Regex::new(r#"\p{Emoji}"#).unwrap();
		for m in emoji_match.find_iter(&raw){
			if m.len()==1{
				continue;
			}
			if let Some((id,url))=unicode_to_emoji(m.as_str(),&emoji_cache.local_instance){
				emoji_indexs.push((m.start(),id,m.len(),url));
				//println!("{}..{}\t{}",m.start(),m.len(),m.as_str());
			}
		}
		//utf絵文字の処理
		emoji_indexs.sort_by(|(a,_,_,_),(b,_,_,_)|a.partial_cmp(b).unwrap());
		let mut offset=0;
		for (idx,id,skip_chars,url) in emoji_indexs{
			if offset<idx{
				let s=&raw[offset..idx];
				render.push(MFMElement::Text(s.to_owned()));
			}
			offset=idx+skip_chars;
			let unique_emoji_id=if id.starts_with(":")&&id.ends_with(":"){
				format!("{}{}",&id[1..id.len()-1],instance_str)
			}else{
				id
			};
			let img=emoji_cache.load(unique_emoji_id,url.as_str()).await;
			render.push(MFMElement::Emoji(img));
		}
		if offset<raw.len(){
			let s=&raw[offset..];
			render.push(MFMElement::Text(s.to_owned()));
		}
		//println!("{:?}",render);
		Self{
			raw,
			render,
		}
	}
	pub fn emojis(&self)->impl Iterator<Item=&Arc<UrlImage>>{
		self.render.iter().map(|s|{
			match s{
				MFMElement::Emoji(img)=>{
					Some(&img.img)
				},
				_=>None
			}
		}).filter(|s|s.is_some()).map(|s|s.unwrap())
	}
	pub fn render(&self,ui:&mut egui::Ui,strong:bool,dummy:&UrlImage,animate_frame:u64){
		ui.horizontal_wrapped(|ui|{
			ui.spacing_mut().item_spacing=[0f32,0f32].into();
			let mut ctx=MFMRenderContext::new();
			for r in &self.render{
				r.render(ui,strong,dummy,&mut ctx,animate_frame);
			}
		});
	}
}
#[derive(Debug)]
pub struct FediverseInstance{
	host:String,
	display_name:String,
	theme_color:Color32,
	pub icon:UrlImage,
}
impl  FediverseInstance{
	pub fn new(value: &RawInstance,media_proxy:impl AsRef<str>) -> Self {
		let RawInstance{
			name,
			favicon_url,
			icon_url,
			theme_color,
			..
		}=value;
		let theme_color=theme_color.as_ref().map(|t|t.as_str()).unwrap_or("#000000");
		fn to_color(theme_color:&str)->Result<Color32,ParseIntError>{
			//println!("theme_color:{}/{}/{}/{}",theme_color,&theme_color[1..3],&theme_color[3..5],&theme_color[5..7]);
			let r=u8::from_str_radix(&theme_color[1..3],16)?;
			let g=u8::from_str_radix(&theme_color[3..5],16)?;
			let b=u8::from_str_radix(&theme_color[5..7],16)?;
			Ok(Color32::from_rgb(r,g,b))
		}
		let theme_color=to_color(theme_color).unwrap_or_default();
		//println!("{:?} {:?}",name,theme_color);

		let icon_url=match favicon_url.as_ref(){
			Some(s)=>Some(s.as_str()),
			None=>match icon_url.as_ref(){
				Some(s)=>Some(s.as_str()),
				None=>None,
			},
		};
		let icon_url=match icon_url{
			Some(icon_url)=>format!("{}/emoji.webp?url={}&emoji=1",media_proxy.as_ref(),urlencoding::encode(icon_url)),
			None=>DUMMY_PNG.to_owned()
		};
		Self{
			host:"".to_owned(),
			display_name:name.clone().unwrap_or_default(),
			theme_color,
			icon:icon_url.into()
		}
	}
	pub fn host(&self)->&str{
		&self.host
	}
	pub fn display_name(&self)->&str{
		&self.display_name
	}
	pub fn theme_color(&self)->Color32{
		self.theme_color
	}
}
enum TextureState{
	OnMemory(Vec<(u32,egui::ColorImage)>),
	OnGpu(Vec<(u32,egui::TextureHandle)>),
	None,
}
impl Debug for TextureState{
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::OnMemory(_) => f.debug_tuple("OnMemory").finish(),
			Self::OnGpu(_) => f.debug_tuple("OnGpu").finish(),
			Self::None => write!(f, "None"),
		}
	}
}
impl TextureState{
	fn take_memory(&mut self)->Option<Vec<(u32,egui::ColorImage)>>{
		if let Self::OnMemory(_)=self{

		}else{
			return None;
		}
		let t=std::mem::take(self);
		match t{
			TextureState::OnMemory(t) => Some(t),
			TextureState::OnGpu(_) => None,
			TextureState::None => None,
		}
	}
	fn take_gpu(&mut self)->Option<Vec<(u32,egui::TextureHandle)>>{
		if let Self::OnGpu(_)=self{

		}else{
			return None;
		}
		let t=std::mem::take(self);
		match t{
			TextureState::OnMemory(_) => None,
			TextureState::OnGpu(t) => Some(t),
			TextureState::None => None,
		}
	}
}
impl From<Vec<(u32,DynamicImage)>> for TextureState{
	fn from(img: Vec<(u32,DynamicImage)>) -> Self {
		let eimg=img.into_iter().map(|(timestamp,img)|{
			let size=[img.width() as _, img.height() as _];
			let image_buffer = img.into_rgba8();
			let pixels = image_buffer.as_flat_samples();
			(timestamp,egui::ColorImage::from_rgba_unmultiplied(
				size,
				pixels.as_slice(),
			))
		}).collect();
		TextureState::OnMemory(eimg)
	}
}
impl Default for TextureState{
	fn default() -> Self {
		Self::None
	}
}
#[derive(Debug)]
pub struct UrlImage{
	url:String,
	img:RwLock<TextureState>,
	loaded:AtomicBool,
}
impl From<String> for UrlImage{
	fn from(url:String) -> Self {
		Self::new(url,TextureState::None)
	}
}
impl UrlImage{
	fn new(url:String,img:TextureState) -> Self {
		let loaded=if let TextureState::OnMemory(_)=&img{
			true
		}else{
			false
		};
		Self{
			url,
			img:RwLock::new(img),
			loaded:AtomicBool::new(loaded),
		}
	}
	pub fn get(&self,animate_ms:u64)->Option<egui::Image<'static>>{
		let r_lock=self.img.blocking_read();
		if let TextureState::OnGpu(h)=&*r_lock{
			let animate_ms=animate_ms as usize;
			let last=h.last()?;
			let animate_ms=if last.0>0{
				animate_ms%(last.0 as usize)
			}else{
				0
			};
			let mut handle=&last.1;
			for (ms,t) in h.iter(){
				if animate_ms>*ms as usize{
					handle=t;
				}
			}
			let tex=egui::load::SizedTexture::from_handle(&handle);
			Some(egui::Image::from_texture(tex))
		}else{
			None
		}
	}
	pub async fn load_gpu(&self,ctx:&egui::Context,config:&ConfigFile){
		let mut r=self.img.write().await;
		let handle=match r.take_memory(){
			Some(mut tex)=>{
				if tex.len()!=0{
					if !config.is_animation.unwrap_or(DEFAULT_ANIMATION){
						let img=tex.remove(0);
						tex.clear();
						tex.push(img);
					}
					let tex:Vec<(u32,egui::TextureHandle)>=tex.into_iter().map(|(timestamp,tex)|{
						let h=ctx.load_texture(&self.url,tex,Default::default());
						(timestamp,h)
					}).collect();
					Some(tex)
				}else{
					*r=TextureState::None;
					None
				}
			},
			None=>None
		};
		if let Some(h)=handle{
			*r=TextureState::OnGpu(h);
		}
	}
	pub fn loaded(&self)->bool{
		self.loaded.load(std::sync::atomic::Ordering::Relaxed)
	}
	pub fn dummy()->Self{
		let img=vec![(0,image::load_from_memory(include_bytes!("dummy.png")).unwrap())];
		let img=RwLock::new(img.into());
		Self{
			url:DUMMY_PNG.to_owned(),
			loaded:AtomicBool::new(true),
			img,
		}
	}
	pub async fn load(&self,client:&reqwest::Client){
		if self.loaded(){
			return;
		}
		if self.url.as_str()==DUMMY_PNG{
			/*
			let icon=include_bytes!("dummy.png");
			if let Ok(img)=image::load_from_memory(icon){
				*self.img.write().await=img.into();
				self.loaded.store(true, std::sync::atomic::Ordering::Relaxed);
			}
			*/
			self.loaded.store(true, std::sync::atomic::Ordering::Relaxed);
			return;
		}
		let resource_id=uuid::Uuid::new_v3(&uuid::Uuid::NAMESPACE_URL,self.url.as_bytes());
		let cache_dir=self::cache_dir();
		if !tokio::fs::try_exists(&cache_dir).await.unwrap_or(true){
			println!("create_dir_all {:?}",tokio::fs::create_dir_all(&cache_dir).await);
		}
		let cache_file=std::path::Path::new(&cache_dir).to_path_buf().join(resource_id.to_string());
		if tokio::fs::try_exists(&cache_file).await.unwrap_or(false){
			if let Ok(mut f)=tokio::fs::File::open(&cache_file).await{
				let mut buf=vec![];
				if f.read_to_end(&mut buf).await.is_ok(){
					println!("GET CACHE-HIT {}",cache_file.as_os_str().to_string_lossy());
					self.load_bytes(&buf).await;
					return;
				}
			}
		}
		eprintln!("GET {}",self.url);
		if let Ok(icon_data)=client.get(&self.url).send().await{
			if !icon_data.status().is_success(){
				eprintln!("Remote status {} {}",icon_data.status(),self.url);
				self.loaded.store(true, std::sync::atomic::Ordering::Relaxed);
				return;
			}
			let mut immutable=false;
			if let Some(Ok(cc))=icon_data.headers().get(reqwest::header::CACHE_CONTROL).map(|s|s.to_str()){
				if cc.contains("immutable"){
					immutable=true;
				}
			}
			if let Ok(icon)=icon_data.bytes().await{
				//ローカルにキャッシュする
				if immutable&&icon.len()<1*1024*1024{//1ファイル1MB上限
					if let Ok(mut f)=tokio::fs::File::create(&cache_file).await{
						if f.write_all(&icon).await.is_err(){
							drop(f);
							let _=tokio::fs::remove_file(&cache_file).await;
						}else{
							println!("GET CACHE-WRITE {}",cache_file.as_os_str().to_string_lossy());
						}
					}
				}
				self.load_bytes(&icon).await;
			}
		}
	}
	async fn load_bytes(&self,icon:&[u8]){
		match image::guess_format(&icon){
			Ok(codec)=>{
				if let image::ImageFormat::WebP=codec{
					let decoder=webp::AnimDecoder::new(&icon);
					match decoder.decode(){
						Ok(image) => {
							*self.img.write().await=image.into_iter().map(|frame|{
								(frame.get_time_ms() as u32,Into::<image::DynamicImage>::into(&frame))
							}).collect::<Vec<_>>().into();
							self.loaded.store(true, std::sync::atomic::Ordering::Relaxed);
							return;
						},
						Err(e) => {
							eprintln!("{:?}",e);
						},
					}
				}
				match image::load_from_memory_with_format(&icon,codec){
					Ok(img)=>{
						*self.img.write().await=vec![(0,img)].into();
						self.loaded.store(true, std::sync::atomic::Ordering::Relaxed);
					},
					Err(e)=>{
						eprintln!("{}",codec.to_mime_type());
						eprintln!("{:?} {}",e,self.url);
						self.loaded.store(true, std::sync::atomic::Ordering::Relaxed);
					}
				}
			},
			Err(e)=>{
				eprintln!("{:?} {}",e,self.url);
				self.loaded.store(true, std::sync::atomic::Ordering::Relaxed);
			}
		}
	}
	pub async fn unload(&self){
		let mut wl=self.img.write().await;
		std::mem::take::<TextureState>(&mut wl);
	}
}
