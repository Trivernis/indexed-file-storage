use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::cmp::Ordering;
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
        if length < 8 {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        let mut name_buf = vec![0u8; (length - 8) as usize];
        reader.read(&mut name_buf)?;
        let name =
            String::from_utf8(name_buf).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
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

    /// Deletes an entry from the chunk if it's contained in it
    pub fn delete_entry<R: Read + Seek, W: Write + Seek>(
        &mut self,
        name: &str,
        reader: &mut R,
        writer: &mut W,
    ) -> io::Result<()> {
        let mut current: usize = 0;
        let mut deleted_size = 0;
        reader.seek(SeekFrom::Start(self.location + 6))?;
        let mut found = false;

        for _ in 0..self.entries {
            let entry = DirEntry::from_reader(reader)?;
            if entry.name == name {
                deleted_size = entry.size();
                found = true;
                break;
            }
            current += entry.size();
        }
        // check if we found the entry
        if !found {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        writer.seek(SeekFrom::Start(current as u64 + self.location + 6))?;
        reader.seek(SeekFrom::Start(
            (current + deleted_size) as u64 + self.location + 6,
        ))?;
        let mut remaining_buf = vec![0u8; (self.length as usize) - (current + deleted_size)];
        reader.read_exact(&mut remaining_buf)?;
        writer.write(&remaining_buf[..])?;
        self.entries -= 1;
        self.write_header(writer)?;

        Ok(())
    }

    pub fn size(&self) -> usize {
        self.length as usize + 8 + 6
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
            let chunk = DirChunk::new(self.get_size()?, CHUNK_SIZE as u32);
            chunk.write_empty(&mut writer)?;
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
                        if entry.child_pointer == 0 {
                            return Err(io::Error::from(ErrorKind::NotFound));
                        }
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

    pub fn has_entry(&mut self, name: &str) -> io::Result<bool> {
        Ok(self.entries()?.iter().find(|e| e.name == name).is_some())
    }

    /// Create a new entry in the current directory
    pub fn create_entry(&mut self, name: &str, dir: bool) -> io::Result<()> {
        if name.contains('/') || name.len() == 0 {
            return Err(io::Error::from(ErrorKind::InvalidData));
        }
        if self.has_entry(name)? {
            return Err(io::Error::from(ErrorKind::AlreadyExists));
        }
        self.create_dir_entry(name, dir)
    }

    /// Deletes an entry in the current directory
    pub fn delete_entry(&mut self, name: &str) -> io::Result<bool> {
        let mut reader = self.get_reader()?;
        let mut chunk = DirChunk::from_reader(self.position, &mut reader)?;
        let mut found = false;

        loop {
            if let Some(_) = chunk.entries(&mut reader)?.iter().find(|e| e.name == name) {
                found = true;
                break;
            }
            if chunk.next == 0 {
                break;
            }
            chunk = DirChunk::from_reader(chunk.next, &mut reader)?;
        }
        if found {
            let mut writer = self.get_writer()?;
            chunk.delete_entry(name, &mut reader, &mut writer)?;
            writer.flush()?;
        }

        Ok(found)
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

    /// Creates a new dir entry without the name check
    fn create_dir_entry(&mut self, name: &str, dir: bool) -> io::Result<()> {
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

    fn memory_layout<R: Read + Seek>(
        &self,
        location: u64,
        reader: &mut R,
    ) -> io::Result<Vec<(u64, u64)>> {
        let mut layout = Vec::new();
        let chunk = DirChunk::from_reader(location, reader)?;
        layout.push((chunk.location, chunk.location + chunk.size() as u64));

        if chunk.next != 0 {
            layout.append(&mut self.memory_layout(chunk.next, reader)?);
        }
        for child in chunk.entries(reader)? {
            if child.child_pointer != 0 {
                layout.append(&mut self.memory_layout(child.child_pointer, reader)?);
            }
        }

        Ok(layout)
    }

    /// Creates a new chunk at the end of the file
    fn new_chunk(&self, writer: &mut BufWriter<File>) -> io::Result<DirChunk> {
        let chunk = DirChunk::new(
            self.next_chunk_location(CHUNK_SIZE as u64)?,
            CHUNK_SIZE as u32,
        );
        chunk.write_empty(writer)?;

        Ok(chunk)
    }

    /// Returns the size of the file in bytes
    pub fn get_size(&self) -> io::Result<u64> {
        self.path.metadata().map(|m| m.len())
    }

    /// Returns the next available chunk location
    fn next_chunk_location(&self, size: u64) -> io::Result<u64> {
        let mut reader = self.get_reader()?;
        let mut layout = self.memory_layout(0, &mut reader)?;
        layout.sort_by(|(a, _), (b, _)| {
            if a > b {
                Ordering::Greater
            } else if a < b {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        });
        let mut previous = 0;

        for (a1, a2) in layout {
            if a1 - previous > size {
                return Ok(previous);
            }
            previous = a2;
        }

        self.get_size()
    }
}
