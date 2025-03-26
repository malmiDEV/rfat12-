use std::{fs::File, io::Read};
use bytemuck::{Pod, Zeroable, NoUninit};

#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
struct FATBootsector {
    jmp_inst: [u8; 3],
    oem: [u8; 8],
    bytes_per_sector: u16,
    sector_per_cluster: u8,
    reserved_sectors: u16,
    fat_count: u8,
    root_dir_ent: u16,
    total_sector: u16,
    media_descriptor_type: u8,
    sector_per_fat: u16,
    sector_per_track: u16,
    heads: u16,
    hidden_sector: u32,
    large_sector_count: u32,

    drive_number: u8,
    reserved: u8,
    signature: u8,
    volume_serial: u32,
    volume_label: [u8; 11],
    system_identifier: [u8; 8],
}

unsafe impl Zeroable for FATBootsector {}
unsafe impl Pod for FATBootsector {}

#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
struct FATDirectoryEntry {
    name: [u8; 11],
    attributes: u8,
    reserved: u8,
    created_time_tenths: u8,
    created_tile: u16,
    created_date: u16,
    last_accessed_data: u16,
    high_16b_entry: u16,
    last_modification_time: u16,
    last_modification_date: u16,
    low_16b_entry: u16,
    size: u32
}

unsafe impl Zeroable for FATDirectoryEntry {}
unsafe impl Pod for FATDirectoryEntry {}

trait FATPrepare {
    fn load_image(path: &str) -> Result<Vec<u8>, String>;
    fn read_bootsector(data: &Vec<u8>) -> Result<FATBootsector, String>;
    fn read_root_directory(disk: &Vec<u8>, header: &FATBootsector) -> Result<(Vec<FATDirectoryEntry>, usize), String>;
    fn read_fat(header: &FATBootsector, disk: &Vec<u8>) -> Result<Vec<u8>, String>;
    fn read_sector<T>(header: &FATBootsector, disk: &Vec<u8>, lba: u32, total: u32) -> Result<Vec<T>, String> 
        where T: FATStruct;
}

trait FATStruct: Sized + Copy {
    fn from_bytes(bytes: &[u8]) -> Option<Self>;
}

impl FATStruct for FATBootsector {
    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() >= std::mem::size_of::<Self>() {
            Some(*bytemuck::from_bytes(bytes))
        } else {
            None
        }
    }
}

impl FATStruct for FATDirectoryEntry {
    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() >= std::mem::size_of::<Self>() {
            Some(*bytemuck::from_bytes(bytes))
        } else {
            None
        }
    }
}

impl FATStruct for u8 {
    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() >= std::mem::size_of::<Self>() {
            Some(*bytemuck::from_bytes(bytes))
        } else {
            None
        }
    }
}

struct FAT12 {
    disk: Vec<u8>,
    bootsector: FATBootsector,
    rootdir: Vec<FATDirectoryEntry>,
    rootdir_end: usize,
    fat: Vec<u8>
}

impl FAT12 {
    pub fn new(path: &str) -> Result<FAT12, String> {
        let disk = Self::load_image(path)?;
        let bootsector = Self::read_bootsector(&disk)?;        
        let (rootdir, rootdir_end) = Self::read_root_directory(&disk, &bootsector)?;
        let fat = Self::read_fat(&bootsector, &disk)?;
        Ok(FAT12 {
            disk,
            bootsector,
            rootdir,
            rootdir_end,
            fat
        })
    }   
    
    pub fn search_file(&mut self, name: &[u8]) -> Option<&FATDirectoryEntry> {
        for i in 0..self.bootsector.root_dir_ent {
            if self.rootdir[i as usize].name == name {
                return Some(&self.rootdir[i as usize])
            }
        }
        None
    }

    pub fn read_file(&mut self, entry: &FATDirectoryEntry) -> Result<Vec<u8>, String> {
        let mut output = vec![];
        let mut cluster = entry.low_16b_entry;
        loop {
            let lba = self.rootdir_end + ((cluster - 2) * self.bootsector.sector_per_cluster as u16) as usize;
            let data = Self::read_sector::<u8>(&self.bootsector, &self.disk, lba as u32, self.bootsector.sector_per_cluster as u32)?;
            output.extend_from_slice(&data);
            let fat_index = (cluster * 3 / 2) as usize;
            if cluster % 2 == 0 {
                cluster = ((self.fat[fat_index] as u16) | ((self.fat[fat_index + 1] as u16) << 8)) & 0x0FFF;
            } else {
                cluster = ((self.fat[fat_index] as u16) >> 4) | ((self.fat[fat_index + 1] as u16) << 4);
            }            
            if cluster > 0x0ff8 { break }
        }
        Ok(output)
    }

    pub fn parse(&mut self, file: &[u8]) -> Result<Vec<u8>, String> {
        if let Some(&e) = self.search_file(file) {
            let content = self.read_file(&e)?;            
            return Ok(content);
        } 
        return Err("Error Parse File".to_string())
    }
}

impl FATPrepare for FAT12 {
    fn load_image(path: &str) -> Result<Vec<u8>, String> {    
        let mut data = Vec::<u8>::new();
        if let Ok(mut f) = File::open(path) {
            f.read_to_end(&mut data)
                .expect(format!("Cannot Read file at: {}", path).as_str());  
        } else {
            return Err("failed to load file".to_string());
        }
        Ok(data)
    }    
    
    fn read_bootsector(data: &Vec<u8>) -> Result<FATBootsector, String> {
        if data.len() < std::mem::size_of::<FATBootsector>() {
            return Err("read_bootsector failed".to_string())
        }
        Ok(*bytemuck::from_bytes(&data[..std::mem::size_of::<FATBootsector>()]))
    }    

    fn read_root_directory(disk: &Vec<u8>, header: &FATBootsector) -> Result<(Vec<FATDirectoryEntry>, usize), String> {
        let lba = header.reserved_sectors + header.sector_per_fat * header.fat_count as u16;
        let size = std::mem::size_of::<FATDirectoryEntry>() as u16 * header.root_dir_ent;
        let sectors = size / header.bytes_per_sector;
        let end = (lba + sectors) as usize;
        let root = Self::read_sector::<FATDirectoryEntry>(header, disk, lba as u32, sectors as u32)?;
        Ok((root, end))
    }

    fn read_fat(header: &FATBootsector, disk: &Vec<u8>) -> Result<Vec<u8>, String> {
        let fat = Self::read_sector::<u8>(header, disk, header.reserved_sectors as u32, header.sector_per_fat as u32)?;
        Ok(fat)
    }

    fn read_sector<T>(header: &FATBootsector, disk: &Vec<u8>, lba: u32, total: u32) -> Result<Vec<T>, String> 
        where T: FATStruct
    {
        let start_pos = (lba * header.bytes_per_sector as u32) as usize;
        let end_pos = (start_pos + total as usize * header.bytes_per_sector as usize) as usize;
        
        if end_pos > disk.len() {
            return Err("read_sector out of bound".to_string())
        }

        let sector_data = &disk[start_pos..end_pos];
        let entry_size = std::mem::size_of::<T>();
        let entries = sector_data
            .chunks_exact(entry_size)
            .filter_map(|chunk| T::from_bytes(chunk))
            .collect::<Vec<T>>();

        Ok(entries)
    }
}

fn main() {
    let mut fat12 = FAT12::new("os.img").expect("err create Fat12");
    println!("{}", String::from_utf8(fat12.parse(b"TEST    TXT").expect("parse error")).expect("msg"));
}
