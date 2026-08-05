use crate::buffer;
use crate::image;
use std::borrow::Cow;
use std::env::current_dir;
use std::fs::File;
use std::io::Cursor;
use std::io::Read;
use std::io::Seek;
use std::{fs, io};

use crate::{Document, Error, Gltf, Result};
use image_crate::ImageFormat;
#[cfg(feature = "EXT_texture_webp")]
use image_crate::ImageFormat::WebP;
use image_crate::ImageFormat::{Jpeg, Png};
use std::path::Path;
use url::Url;

/// Return type of `import`.
type Import = (Document, Vec<buffer::Data>, Vec<image::Data>);

/// TODO
pub trait Importer: Sized {
    /// TODO
    fn open(&self, uri: &Url) -> Result<impl Resource>;
    /// TODO
    fn base(&self) -> &Url;
}

/// TODO
pub trait ImporterExt: Importer {
    /// TODO
    fn resolve(&self, uri: &str, base: Option<&Url>) -> Result<Url> {
        // TODO: the error here should probably be related to the slice thingy
        base.unwrap_or(self.base()).join(uri).map_err(Error::Uri)
    }

    /// TODO
    fn import(&self, uri: &str) -> Result<Import> {
        let uri = self.resolve(uri, None)?;
        let resource = self.open(&uri)?;
        let document = Gltf::from_reader(resource.seekable())?;
        self.import_resources(document, Some(&uri))
    }

    /// TODO
    fn import_buffers(
        &self,
        document: &Document,
        base: Option<&Url>,
        mut blob: Option<Vec<u8>>,
    ) -> Result<Vec<buffer::Data>> {
        let mut buffers = Vec::new();
        for buffer in document.buffers() {
            let data =
                buffer::Data::import_from_source_and_blob(self, buffer.source(), base, &mut blob)?;
            if data.len() < buffer.length() {
                return Err(Error::BufferLength {
                    buffer: buffer.index(),
                    expected: buffer.length(),
                    actual: data.len(),
                });
            }
            buffers.push(data);
        }
        Ok(buffers)
    }

    /// TODO: doc
    fn import_images(
        &self,
        document: &Document,
        base: Option<&Url>,
        buffer_data: &[buffer::Data],
    ) -> Result<Vec<image::Data>> {
        let mut images = Vec::new();
        for image in document.images() {
            images.push(image::Data::import_from_source(
                self,
                image.source(),
                base,
                buffer_data,
            )?);
        }
        Ok(images)
    }

    /// TODO: doc
    fn import_resources(
        &self,
        Gltf { document, blob }: Gltf,
        base: Option<&Url>,
    ) -> Result<Import> {
        let buffer_data = self.import_buffers(&document, base, blob)?;
        let image_data = self.import_images(&document, base, &buffer_data)?;
        let import = (document, buffer_data, image_data);
        Ok(import)
    }
}

impl<T: Importer> ImporterExt for T {}

pub trait Resource: Read {
    fn media_type(&self) -> Option<&str>;
    fn seekable(self) -> impl Read + Seek;

    fn into_bytes(mut self) -> io::Result<Vec<u8>>
    where
        Self: Sized,
    {
        let mut buf = Vec::new();
        Read::read_to_end(&mut self, &mut buf)?;
        Ok(buf)
    }
}

/// TODO: doc
pub struct DataResource<'a> {
    media_type: Option<&'a str>,
    data: Cursor<Vec<u8>>,
}

impl Read for DataResource<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.data.read(buf)
    }
}

impl Seek for DataResource<'_> {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        self.data.seek(pos)
    }
}

/// TODO: doc
pub struct FileResource<'a> {
    extension: Option<&'a str>,
    file: File,
}

enum DefaultResource<'a> {
    Data(DataResource<'a>),
    File(FileResource<'a>),
}

impl Read for FileResource<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }
}

impl Seek for FileResource<'_> {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }
}

impl Resource for DataResource<'_> {
    fn media_type(&self) -> Option<&str> {
        self.media_type
    }

    fn seekable(self) -> impl Read + Seek {
        self
    }
}

impl Resource for FileResource<'_> {
    fn media_type(&self) -> Option<&str> {
        match self.extension {
            Some("jpg") | Some("jpeg") => Some("image/jpeg"),
            Some("png") => Some("image/png"),
            #[cfg(feature = "EXT_texture_webp")]
            Some("webp") => Some("image/webp"),
            _ => None,
        }
    }

    fn seekable(self) -> impl Read + Seek {
        self
    }
}

impl Read for DefaultResource<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            DefaultResource::Data(data) => data.read(buf),
            DefaultResource::File(file) => file.read(buf),
        }
    }
}

impl Seek for DefaultResource<'_> {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        match self {
            DefaultResource::Data(data) => data.seek(pos),
            DefaultResource::File(file) => file.seek(pos),
        }
    }
}

impl Resource for DefaultResource<'_> {
    fn media_type(&self) -> Option<&str> {
        match self {
            DefaultResource::Data(data) => data.media_type(),
            DefaultResource::File(file) => file.media_type(),
        }
    }

    fn seekable(self) -> impl Read + Seek {
        self
    }
}

impl<'a> TryFrom<&'a Url> for DataResource<'a> {
    type Error = Error;
    fn try_from(value: &'a Url) -> Result<Self> {
        let mut it = value.path().split(";base64,");
        let (media_type, encoded) = match (it.next(), it.next()) {
            (match0_opt, Some(match1)) => (match0_opt, match1),
            (Some(match0), _) => (None, match0),
            _ => return Err(Error::UnsupportedScheme),
        };
        Ok(DataResource {
            media_type,
            data: Cursor::new(base64::decode(encoded).map_err(Error::Base64)?),
        })
    }
}

impl<'a> TryFrom<&'a Url> for FileResource<'a> {
    type Error = Error;
    fn try_from(value: &'a Url) -> Result<Self> {
        let path: &Path = value.path().as_ref();
        let extension = path.extension().and_then(|s| s.to_str());

        Ok(FileResource {
            extension,
            file: File::open(path).map_err(Error::Io)?,
        })
    }
}

pub struct DefaultImporter {
    base: Url,
}

impl Default for DefaultImporter {
    fn default() -> Self {
        let base =
            Url::from_directory_path(current_dir().expect("failed to get current directory"))
                .expect("failed to make URL for current directory");
        Self { base }
    }
}

impl Importer for DefaultImporter {
    fn open(&self, uri: &Url) -> Result<impl Resource> {
        match uri.scheme() {
            "data" => Ok(DefaultResource::Data(DataResource::try_from(uri)?)),
            "file" => Ok(DefaultResource::File(FileResource::try_from(uri)?)),
            _ => Err(Error::UnsupportedScheme),
        }
    }

    fn base(&self) -> &Url {
        &self.base
    }
}

fn read_to_end(resource: &mut impl Resource) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    Read::read_to_end(resource, &mut buf)?;
    Ok(buf)
}

fn path_to_uri(path: Option<&Path>) -> Result<Option<Url>> {
    path.map(Url::from_file_path)
        .transpose()
        .map_err(|_| Error::UnsupportedScheme)
}

impl buffer::Data {
    /// Construct a buffer data object by reading the given source.
    /// If `base` is provided, then external filesystem references will
    /// be resolved from this directory.
    pub fn from_source(source: buffer::Source<'_>, base: Option<&Path>) -> Result<Self> {
        Self::from_source_and_blob(source, base, &mut None)
    }

    /// Construct a buffer data object by reading the given source.
    /// If `base` is provided, then external filesystem references will
    /// be resolved from this directory.
    /// `blob` represents the `BIN` section of a binary glTF file,
    /// and it will be taken to fill the buffer if the `source` refers to it.
    pub fn from_source_and_blob(
        source: buffer::Source<'_>,
        base: Option<&Path>,
        blob: &mut Option<Vec<u8>>,
    ) -> Result<Self> {
        Self::import_from_source_and_blob(
            &DefaultImporter::default(),
            source,
            path_to_uri(base)?.as_ref(),
            blob,
        )
    }

    /// TODO
    pub fn import_from_source_and_blob(
        importer: &impl Importer,
        source: buffer::Source<'_>,
        base: Option<&Url>,
        blob: &mut Option<Vec<u8>>,
    ) -> Result<Self> {
        let mut data = match source {
            buffer::Source::Uri(uri) => importer
                .open(&importer.resolve(uri, base)?)?
                .into_bytes()
                .map_err(Error::Io),
            buffer::Source::Bin => blob.take().ok_or(Error::MissingBlob),
        }?;
        while data.len() % 4 != 0 {
            data.push(0);
        }
        Ok(buffer::Data(data))
    }
}

/// Import buffer data referenced by a glTF document.
///
/// ### Note
///
/// This function is intended for advanced users who wish to forego loading image data.
/// A typical user should call [`import`] instead.
pub fn import_buffers(
    document: &Document,
    base: Option<&Path>,
    blob: Option<Vec<u8>>,
) -> Result<Vec<buffer::Data>> {
    let base = path_to_uri(base)?;
    let importer = DefaultImporter::default();
    importer.import_buffers(document, base.as_ref(), blob)
}

impl image::Data {
    /// Construct an image data object by reading the given source.
    /// If `base` is provided, then external filesystem references will
    /// be resolved from this directory.
    pub fn from_source(
        source: image::Source<'_>,
        base: Option<&Path>,
        buffer_data: &[buffer::Data],
    ) -> Result<Self> {
        Self::import_from_source(
            &DefaultImporter::default(),
            source,
            path_to_uri(base)?.as_ref(),
            buffer_data,
        )
    }

    /// TODO
    pub fn import_from_source(
        importer: &impl Importer,
        source: image::Source<'_>,
        base: Option<&Url>,
        buffer_data: &[buffer::Data],
    ) -> Result<Self> {
        #[cfg(feature = "guess_mime_type")]
        let guess_format = |encoded_image: &[u8]| match image_crate::guess_format(encoded_image) {
            Ok(image_crate::ImageFormat::Png) => Some(Png),
            Ok(image_crate::ImageFormat::Jpeg) => Some(Jpeg),
            #[cfg(feature = "EXT_texture_webp")]
            Ok(image_crate::ImageFormat::WebP) => Some(WebP),
            _ => None,
        };
        #[cfg(not(feature = "guess_mime_type"))]
        let guess_format = |_encoded_image: &[u8]| None;

        let choose_format =
            |mime_type: Option<&str>, encoded_image: &[u8]| -> Result<ImageFormat> {
                match mime_type {
                    Some("image/png") => Ok(Png),
                    Some("image/jpeg") => Ok(Jpeg),
                    #[cfg(feature = "EXT_texture_webp")]
                    Some("image/webp") => Ok(WebP),
                    _ => guess_format(&encoded_image).ok_or(Error::UnsupportedImageEncoding)?,
                }
            };

        let (encoded_image, encoded_format) = match source {
            image::Source::Uri { uri, mime_type } => {
                let uri = importer.resolve(uri, base)?;
                let mut resource = importer.open(&uri)?;
                let encoded_image = read_to_end(&mut resource)?;
                let mime_type = resource.media_type().or(mime_type);
                let encoded_format = choose_format(mime_type, &encoded_image)?;

                (Cow::from(encoded_image), encoded_format)
            }
            image::Source::View { view, mime_type } => {
                let parent_buffer_data = &buffer_data[view.buffer().index()].0;
                let begin = view.offset();
                let end = begin + view.length();
                let encoded_image = &parent_buffer_data[begin..end];
                let encoded_format = choose_format(Some(mime_type), encoded_image)?;

                (Cow::from(encoded_image), encoded_format)
            }
        };

        let decoded_image =
            image_crate::load_from_memory_with_format(&encoded_image, encoded_format)?;
        image::Data::new(decoded_image)
    }
}

/// Import image data referenced by a glTF document.
///
/// ### Note
///
/// This function is intended for advanced users who wish to forego loading buffer data.
/// A typical user should call [`import`] instead.
pub fn import_images(
    document: &Document,
    base: Option<&Path>,
    buffer_data: &[buffer::Data],
) -> Result<Vec<image::Data>> {
    let base = path_to_uri(base)?;
    let importer = DefaultImporter::default();
    importer.import_images(document, base.as_ref(), buffer_data)
}

fn import_path(path: &Path) -> Result<Import> {
    let importer = DefaultImporter::default();
    let file = fs::File::open(path).map_err(Error::Io)?;
    let reader = io::BufReader::new(file);
    let gltf = Gltf::from_reader(reader)?;
    importer.import_resources(gltf, path_to_uri(Some(path))?.as_ref())
}

/// Import glTF 2.0 from the file system.
///
/// ```
/// # fn run() -> Result<(), gltf::Error> {
/// # let path = "examples/Box.gltf";
/// # #[allow(unused)]
/// let (document, buffers, images) = gltf::import(path)?;
/// # Ok(())
/// # }
/// # fn main() {
/// #     run().expect("test failure");
/// # }
/// ```
///
/// ### Note
///
/// This function is provided as a convenience for loading glTF and associated
/// resources from the file system. It is suitable for real world use but may
/// not be suitable for all real world use cases. More complex import scenarios
/// such downloading from web URLs are not handled by this function. These
/// scenarios are delegated to the user.
///
/// You can read glTF without loading resources by constructing the [`Gltf`]
/// (standard glTF) or [`Glb`] (binary glTF) data structures explicitly.
///
/// [`Gltf`]: struct.Gltf.html
/// [`Glb`]: struct.Glb.html
pub fn import<P>(path: P) -> Result<Import>
where
    P: AsRef<Path>,
{
    import_path(path.as_ref())
}

fn import_slice_impl(slice: &[u8]) -> Result<Import> {
    let importer = DefaultImporter::default();
    let gltf = Gltf::from_slice(slice)?;
    importer.import_resources(gltf, None)
}

/// Import glTF 2.0 from a slice.
///
/// File paths in the document are assumed to be relative to the current working
/// directory.
///
/// ### Note
///
/// This function is intended for advanced users.
/// A typical user should call [`import`] instead.
///
/// ```
/// # extern crate gltf;
/// # use std::fs;
/// # use std::io::Read;
/// # fn run() -> Result<(), gltf::Error> {
/// # let path = "examples/Box.glb";
/// # let mut file = fs::File::open(path).map_err(gltf::Error::Io)?;
/// # let mut bytes = Vec::new();
/// # file.read_to_end(&mut bytes).map_err(gltf::Error::Io)?;
/// # #[allow(unused)]
/// let (document, buffers, images) = gltf::import_slice(bytes.as_slice())?;
/// # Ok(())
/// # }
/// # fn main() {
/// #     run().expect("test failure");
/// # }
/// ```
pub fn import_slice<S>(slice: S) -> Result<Import>
where
    S: AsRef<[u8]>,
{
    import_slice_impl(slice.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
}
