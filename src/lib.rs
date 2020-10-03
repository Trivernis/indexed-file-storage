pub mod metafile;
pub mod storage;
pub mod utils;

#[cfg(test)]
mod tests {
    use crate::metafile::IndexedMetaFile;
    use std::io;

    #[test]
    fn it_writes_meta_files() -> io::Result<()> {
        let mut meta_file = IndexedMetaFile::new()?;
        meta_file.add_entry("./example-file.txt", 0, 1);
        meta_file.add_entry("./example2-file.png", 2, 4);
        let mut result = Vec::with_capacity(0);
        meta_file.write(&mut result)?;
        println!("{:?}", result);
        assert_eq!(result[4..8], [0, 0, 0, 2]);

        Ok(())
    }

    #[test]
    fn it_reads_meta_files() -> io::Result<()> {
        let data = vec![
            0, 0, 0, 0, 0, 0, 0, 2, 202, 81, 124, 83, 81, 43, 20, 236, 144, 180, 132, 124, 159,
            205, 19, 26, 140, 136, 212, 70, 131, 98, 133, 3, 162, 59, 219, 124, 6, 83, 151, 22, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 203, 211, 57, 78, 186, 86, 131, 6, 119, 69, 122, 247,
            249, 70, 190, 243, 51, 250, 52, 174, 16, 65, 62, 221, 187, 212, 38, 92, 31, 58, 51,
            174, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 4,
        ];
        let meta_file = IndexedMetaFile::from_reader(&data[..])?;
        assert_eq!(meta_file.get_entry("./example-file.txt"), Some(&(0, 1)));

        Ok(())
    }
}
