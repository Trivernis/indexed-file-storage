use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::borrow::Cow;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

const CHUNK_SIZE: u64 = 1024;

#[derive(Clone, Debug)]
pub struct DirEntry {
    pub name: String,
    pub(crate) child_pointer: u64,
}

impl DirEntry {
    pub fn new(name: String, child_pointer: u64) -> Self {
        Self {
            name,
            child_pointer,
        }
    }

    pub fn from_reader<R: Read + Seek>(reader: &mut R) -> io::Result<Self> {
        let length = reader.read_u16::<BigEndian>()?;
        let mut name_buf = vec![0u8; (length - 8) as usize];
        reader.read(&mut name_buf)?;
        let name =
            String::from_utf8(name_buf).map_err(|e| io::Error::from(io::ErrorKind::InvalidData))?;
        let pointer = reader.read_u64::<BigEndian>()?;

        Ok(Self {
            name,
            child_pointer: pointer,
        })
    }

    pub fn write<W: Write + Seek>(&self, writer: &mut W) -> io::Result<usize> {
        let name_raw = self.name.as_bytes();
        writer.write_u16::<BigEndian>(name_raw.len() as u16 + 8)?;
        writer.write(&name_raw)?;
        writer.write_u64::<BigEndian>(self.child_pointer)?;

        Ok((name_raw.len() as u16 + 18) as usize)
    }

    /// Returns the required size for the entry
    pub fn size(&self) -> usize {
        self.name.as_bytes().len() + 10
    }

    pub fn is_dir(&self) -> bool {
        self.child_pointer != 0
    }
}

#[derive(Clone, Debug)]
pub struct DirChunk {
    pub location: u64,
    pub length: u32,
    pub entries: u16,
    pub next: u64,
}

impl DirChunk {
    pub fn new(location: u64, length: u32) -> Self {
        Self {
            location,
            length,
            entries: 0,
            next: 0,
        }
    }

    pub fn from_reader<R: Read + Seek>(location: u64, reader: &mut R) -> io::Result<Self> {
        reader.seek(SeekFrom::Start(location))?;
        let length = reader.read_u32::<BigEndian>()?;
        let entries = reader.read_u16::<BigEndian>()?;
        reader.seek(SeekFrom::Current(length as i64))?;
        let next = reader.read_u64::<BigEndian>()?;
        Ok(Self {
            location,
            length,
            entries,
            next,
        })
    }

    /// Writes the header of the chunk
    pub fn write_header<W: Write + Seek>(&self, writer: &mut W) -> io::Result<()> {
        writer.seek(SeekFrom::Start(self.location))?;
        writer.write_u32::<BigEndian>(self.length)?;
        writer.write_u16::<BigEndian>(self.entries)?;

        Ok(())
    }

    /// Writes the pointer to the next chunk
    pub fn write_next_pointer<W: Write + Seek>(&self, writer: &mut W) -> io::Result<()> {
        writer.seek(SeekFrom::Start(self.location + self.length as u64 + 6))?;
        writer.write_u64::<BigEndian>(self.next)?;

        Ok(())
    }

    /// Writes the empty chunk to the disk
    pub fn write_empty<W: Write + Seek>(&self, writer: &mut W) -> io::Result<()> {
        self.write_header(writer)?;
        let empty_content = vec![0u8; self.length as usize];
        writer.write(&empty_content[..])?;
        writer.write_u64::<BigEndian>(self.next)?;

        Ok(())
    }

    /// Returns all entries in the chunk
    pub fn entries<R: Read + Seek>(&self, reader: &mut R) -> io::Result<Vec<DirEntry>> {
        let mut entries = Vec::new();
        reader.seek(SeekFrom::Start(self.location + 6))?;
        for _ in 0..self.entries {
            entries.push(DirEntry::from_reader(reader)?);
        }

        Ok(entries)
    }

    /// Scans for free space and returns the amount of space as well as the pointer to the write location
    pub fn free_space<R: Read + Seek>(&self, reader: &mut R) -> io::Result<(u32, u64)> {
        let mut current: usize = 0;
        reader.seek(SeekFrom::Start(self.location + 6))?;

        for _ in 0..self.entries {
            let length = reader.read_u16::<BigEndian>()?;
            reader.seek(SeekFrom::Current(length as i64))?;
            current += length as usize + 2;
        }
        let available = self.length - current as u32;

        Ok((available, self.location + 6 + current as u64))
    }
}

pub struct DirTreeFile {
    path: PathBuf,
    dir: Vec<String>,
    position: u64,
    entries: Option<Vec<DirEntry>>,
}

impl DirTreeFile {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            dir: Vec::new(),
            position: 0,
            entries: None,
        }
    }

    pub fn init(&self) -> io::Result<()> {
        if !self.path.exists() || self.get_size()? == 0 {
            let mut writer = self.get_writer()?;
            self.new_chunk(&mut writer)?;
            writer.flush()?;
        }

        Ok(())
    }

    pub fn dir(&self) -> String {
        format!("/{}", self.dir.join("/"))
    }

    /// Reads all entries in the current dir
    pub fn entries(&mut self) -> io::Result<Vec<DirEntry>> {
        if let Some(entries) = self.entries.clone() {
            return Ok(entries);
        }
        let mut reader = self.get_reader()?;
        reader.seek(SeekFrom::Start(self.position))?;
        let mut entries = Vec::new();
        let mut position = self.position;

        loop {
            let chunk = DirChunk::from_reader(position, &mut reader)?;
            entries.append(&mut chunk.entries(&mut reader)?);

            if chunk.next == 0 {
                break;
            }
            position = chunk.next;
        }
        self.entries = Some(entries.clone());

        Ok(entries)
    }

    /// Changes the virtual directory to <dir>
    pub fn cd(&mut self, mut dir: &str) -> io::Result<()> {
        if dir.starts_with('/') {
            self.position = 0;
            self.dir.clear();
            self.entries = None;
            dir = dir.trim_start_matches('/');
        }
        if dir.len() > 0 {
            let parts = dir.split('/');

            for part in parts {
                if part == ".." {
                    self.dir.pop();
                    self.cd(self.dir().as_str())?;
                } else {
                    let entries = self.entries()?;
                    let entry = entries.iter().find(|e| e.name == part);

                    if let Some(entry) = entry {
                        self.position = entry.child_pointer;
                        self.dir.push(part.to_string());
                        self.entries = None;
                    } else {
                        return Err(io::Error::from(ErrorKind::NotFound));
                    }
                }
            }
        }

        Ok(())
    }

    /// Create a new entry in the current directory
    pub fn create_entry(&mut self, name: &str, dir: bool) -> io::Result<()> {
        if name.contains('/') {
            return Err(io::Error::from(ErrorKind::InvalidData));
        }
        if let Some(_) = self.entries()?.iter().find(|e| e.name == name) {
            return Err(io::Error::from(ErrorKind::AlreadyExists));
        }
        let mut reader = self.get_reader()?;
        let mut writer = self.get_writer()?;

        let pointer = if dir {
            self.new_chunk(&mut writer)?.location
        } else {
            0
        };
        let entry = DirEntry::new(name.to_string(), pointer);
        let (mut chunk, write_pointer) = self.find_free_space(entry.size() as u32, &mut reader)?;
        let mut writer = self.get_writer()?;
        writer.seek(SeekFrom::Start(write_pointer))?;
        entry.write(&mut writer)?;
        chunk.entries += 1;
        chunk.write_header(&mut writer)?;
        writer.flush()?;
        if let Some(entries) = &mut self.entries {
            entries.push(entry);
        }

        Ok(())
    }

    fn get_reader(&self) -> io::Result<BufReader<File>> {
        Ok(BufReader::new(File::open(&self.path)?))
    }

    fn get_writer(&self) -> io::Result<BufWriter<File>> {
        Ok(BufWriter::new(
            OpenOptions::new()
                .create(true)
                .write(true)
                .open(&self.path)?,
        ))
    }

    /// Finds free space to write an entry to
    fn find_free_space<R: Read + Seek>(
        &self,
        amount: u32,
        reader: &mut R,
    ) -> io::Result<(DirChunk, u64)> {
        let write_pointer;
        let mut chunk = DirChunk::from_reader(self.position, reader)?;

        loop {
            let (free_amount, pointer) = chunk.free_space(reader)?;
            if free_amount > amount {
                write_pointer = pointer;
                break;
            }

            let next = chunk.next;
            if next == 0 {
                let mut writer = self.get_writer()?;
                let new_chunk = self.new_chunk(&mut writer)?;
                write_pointer = new_chunk.location + 6;
                chunk.next = new_chunk.location;
                chunk.write_next_pointer(&mut writer)?;
                writer.flush()?;
                chunk = new_chunk;
                break;
            }
            chunk = DirChunk::from_reader(next, reader)?;
        }

        Ok((chunk, write_pointer))
    }

    /// Creates a new chunk at the end of the file
    fn new_chunk(&self, writer: &mut BufWriter<File>) -> io::Result<DirChunk> {
        let chunk = DirChunk::new(self.get_size()?, CHUNK_SIZE as u32);
        chunk.write_empty(writer)?;

        Ok(chunk)
    }

    /// Returns the size of the file in bytes
    pub fn get_size(&self) -> io::Result<u64> {
        self.path.metadata().map(|m| m.len())
    }
}
