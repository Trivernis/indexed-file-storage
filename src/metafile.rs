use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{self, Read, Write};

const HASH_SIZE: usize = 256 / 8;

pub type EntryID = [u8; HASH_SIZE];
pub type MetaEntry = (u32, u64);

pub struct IndexedMetaFile {
    entries: HashMap<EntryID, MetaEntry>,
}

impl IndexedMetaFile {
    /// Creates a new indexed meta file assuming it already exists
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            entries: HashMap::new(),
        })
    }

    /// Creates a new MetaFile from a reader
    pub fn from_reader<R: Read>(mut reader: R) -> io::Result<Self> {
        let table_size = reader.read_u64::<BigEndian>()?;
        let entries = Self::read_entries(table_size, reader)?;

        Ok(Self { entries })
    }

    fn read_entries<R: Read>(
        number: u64,
        mut reader: R,
    ) -> io::Result<HashMap<EntryID, MetaEntry>> {
        let mut entries = HashMap::new();
        for _ in 0..number {
            let mut id = [0u8; HASH_SIZE];
            reader.read(&mut id)?;
            let data_file = reader.read_u32::<BigEndian>()?;
            let data_pointer = reader.read_u64::<BigEndian>()?;
            entries.insert(id, (data_file, data_pointer));
        }

        Ok(entries)
    }

    /// Writes the lookup table
    pub fn write<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_u64::<BigEndian>(self.entries.len() as u64)?;
        for (k, (df, dp)) in &self.entries {
            writer.write(k)?;
            writer.write_u32::<BigEndian>(*df)?;
            writer.write_u64::<BigEndian>(*dp)?;
        }

        Ok(())
    }

    /// Adds a file entry
    pub fn add_entry(&mut self, id: &str, file: u32, pointer: u64) {
        self.entries.insert(hash_id(id), (file, pointer));
    }

    /// Returns an entry by id
    pub fn get_entry(&self, id: &str) -> Option<&MetaEntry> {
        self.entries.get(&hash_id(id))
    }

    /// Removes an entry from the meta file
    pub fn remove_entry(&mut self, id: &str) {
        self.entries.remove(&hash_id(id));
    }
}

fn hash_id(id: &str) -> [u8; HASH_SIZE] {
    let mut hasher = Sha256::default();
    hasher.update(&id.as_bytes());
    let result = hasher.finalize();
    let mut array_result = [0u8; HASH_SIZE];
    array_result.copy_from_slice(&result[..]);

    array_result
}
