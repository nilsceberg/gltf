use crate::buffer;
use crate::image;
use std::borrow::Cow;
use std::fs;
use std::fs::File;
use std::io::BufReader;
use std::io::Cursor;
use std::io::Read;
use std::io::Seek;
use std::path::PathBuf;

use crate::{Document, Error, Gltf, Result};
use image_crate::ImageFormat;
#[cfg(feature = "EXT_texture_webp")]
use image_crate::ImageFormat::WebP;
use image_crate::ImageFormat::{Jpeg, Png};
use std::path::Path;

// NOTES:
// * The change in https://github.com/gltf-rs/gltf/pull/279
//   doesn't seem relevant after https://github.com/rust-lang/rust/pull/89165,
//   so we should trust the standard implementations of read_to_end.
//
// * I think there is now an extra allocation when reading from a data URI with read_to_end.
//   Maybe we can do some special casing for BytesOrReader::Bytes if the destination buffer is empty?

/// Return type of `import`.
type Import = (Document, Vec<buffer::Data>, Vec<image::Data>);

/// TODO
pub struct Importer<S> {
    scheme_handler: S,
}

/// TODO
pub trait ResourceResolver<'a> {
    /// TODO
    type Reference<'b>: ResourceReference;

    /// TODO
    fn parse<'b>(uri: &'b str) -> Self::Reference<'b>;

    /// TODO
    fn with_base_from(resource: &'a Self::Reference<'a>) -> Self;

    /// TODO
    fn open<'b>(&'b self, reference: &Self::Reference<'b>) -> Result<impl Read>;

    /// TODO
    fn open_seekable<'b>(&'b self, reference: &Self::Reference<'b>) -> Result<impl Read + Seek> {
        let mut bytes = Vec::new();
        self.open(reference)?.read_to_end(&mut bytes)?;
        Ok(Cursor::new(bytes))
    }
}

/// TODO
pub trait ResourceReference {
    /// TODO
    fn media_type(&self) -> Option<&str>;
    /// TODO
    fn extension(&self) -> Option<&str>;
}

/// TODO
pub struct DefaultReferenceResolver<'a> {
    base: Option<&'a Path>,
}

/// TODO
impl<'a> DefaultReferenceResolver<'a> {
    /// TODO
    pub fn with_base(mut self, base: &'a Path) -> Self {
        self.base = Some(base);
        self
    }
}

impl Default for DefaultReferenceResolver<'_> {
    fn default() -> Self {
        Self { base: None }
    }
}

enum BytesOrReader<R> {
    Bytes(Cursor<Vec<u8>>),
    Reader(R),
}

impl<R> From<Vec<u8>> for BytesOrReader<R> {
    fn from(bytes: Vec<u8>) -> Self {
        BytesOrReader::Bytes(Cursor::new(bytes))
    }
}

impl<R: Read> Read for BytesOrReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            BytesOrReader::Bytes(bytes) => bytes.read(buf),
            BytesOrReader::Reader(reader) => reader.read(buf),
        }
    }
}

impl<R: Seek> Seek for BytesOrReader<R> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        match self {
            BytesOrReader::Bytes(reader) => reader.seek(pos),
            BytesOrReader::Reader(reader) => reader.seek(pos),
        }
    }
}

impl<'a> ResourceResolver<'a> for DefaultReferenceResolver<'a> {
    type Reference<'b> = DefaultScheme<'b>;

    fn parse<'b>(uri: &'b str) -> DefaultScheme<'b> {
        DefaultScheme::parse(uri)
    }

    fn with_base_from(reference: &'a DefaultScheme<'a>) -> Self {
        Self {
            base: reference.base(),
        }
    }

    fn open<'b>(&'b self, scheme: &DefaultScheme<'b>) -> Result<impl Read> {
        self.open_seekable(scheme)
    }

    fn open_seekable(&self, scheme: &DefaultScheme<'_>) -> Result<impl Read + Seek> {
        match scheme {
            // The path may be unused in the Scheme::Data case
            // Example: "uri" : "data:application/octet-stream;base64,wsVHPgA...."
            DefaultScheme::Data(_, base64) => base64::decode(base64)
                .map_err(Error::Base64)
                .map(BytesOrReader::from),
            DefaultScheme::File(path) if self.base.is_some() => {
                Ok(BytesOrReader::Reader(File::open(path)?))
            }
            DefaultScheme::Relative(path) if self.base.is_some() => Ok(BytesOrReader::Reader(
                File::open(self.base.unwrap().join(&**path))?,
            )),
            DefaultScheme::Unsupported => Err(Error::UnsupportedScheme),
            _ => Err(Error::ExternalReferenceInSliceImport),
        }
    }
}

/// Represents the set of URI schemes the importer supports.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum DefaultScheme<'a> {
    /// `data:[<media type>];base64,<data>`.
    Data(Option<&'a str>, &'a str),

    /// `file:[//]<absolute file path>`.
    ///
    /// Note: The file scheme does not implement authority.
    File(&'a Path),

    /// `../foo`, etc.
    Relative(Cow<'a, Path>),

    /// Placeholder for an unsupported URI scheme identifier.
    Unsupported,
}

impl ResourceReference for DefaultScheme<'_> {
    fn media_type(&self) -> Option<&str> {
        match self {
            DefaultScheme::Data(media_type, _) => *media_type,
            _ => None,
        }
    }

    fn extension(&self) -> Option<&str> {
        match self {
            DefaultScheme::File(path) => path.extension(),
            DefaultScheme::Relative(path) => path.extension(),
            _ => None,
        }
        .and_then(|x| x.to_str())
    }
}

impl<'a> DefaultScheme<'a> {
    /// TODO
    pub fn parse(uri: &str) -> DefaultScheme<'_> {
        if uri.contains(':') {
            if let Some(rest) = uri.strip_prefix("data:") {
                let mut it = rest.split(";base64,");

                match (it.next(), it.next()) {
                    (match0_opt, Some(match1)) => DefaultScheme::Data(match0_opt, match1),
                    (Some(match0), _) => DefaultScheme::Data(None, match0),
                    _ => DefaultScheme::Unsupported,
                }
            } else if let Some(rest) = uri.strip_prefix("file://") {
                DefaultScheme::File(rest.as_ref())
            } else if let Some(rest) = uri.strip_prefix("file:") {
                DefaultScheme::File(rest.as_ref())
            } else {
                DefaultScheme::Unsupported
            }
        } else {
            match urlencoding::decode(uri) {
                Ok(decoded) => DefaultScheme::Relative(match decoded {
                    Cow::Borrowed(decoded) => Cow::Borrowed(decoded.as_ref()),
                    Cow::Owned(decoded) => Cow::Owned(PathBuf::from(decoded)),
                }),
                Err(_) => DefaultScheme::Unsupported,
            }
        }
    }

    /// TODO
    pub fn path(&self) -> Option<&Path> {
        match self {
            DefaultScheme::File(path) => Some(path),
            DefaultScheme::Relative(path) => Some(path.as_ref()),
            _ => None,
        }
    }

    /// TODO
    pub fn base(&self) -> Option<&Path> {
        self.path()
            .map(|path| path.parent().unwrap_or("./".as_ref()))
    }
}

impl<S> Importer<S> {
    /// TODO
    pub fn new(scheme_handler: S) -> Self {
        Self { scheme_handler }
    }
}

impl<'a> Importer<DefaultReferenceResolver<'a>> {
    /// TODO
    pub fn with_base(base: Option<&'a Path>) -> Self {
        Self::new(DefaultReferenceResolver { base })
    }
}

impl<'a, S> Importer<S>
where
    S: ResourceResolver<'a>,
{
    /// TODO
    pub fn with_base_from(reference: &'a S::Reference<'a>) -> Self {
        Self::new(S::with_base_from(reference))
    }

    /// TODO
    pub fn import_buffers(
        &self,
        document: &Document,
        mut blob: Option<Vec<u8>>,
    ) -> Result<Vec<buffer::Data>> {
        let mut buffers = Vec::new();
        for buffer in document.buffers() {
            let data = buffer::Data::import_from_source_and_blob(self, buffer.source(), &mut blob)?;
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

    /// TODO
    pub fn import_images(
        &self,
        document: &Document,
        buffer_data: &[buffer::Data],
    ) -> Result<Vec<image::Data>> {
        let mut images = Vec::new();
        for image in document.images() {
            images.push(image::Data::import_from_source(
                self,
                image.source(),
                buffer_data,
            )?);
        }
        Ok(images)
    }

    /// TODO
    pub fn import(&'a self, uri: &'a str) -> Result<Import> {
        self.import_reference(&S::parse(uri))
    }

    /// TODO
    pub fn import_reference(&'a self, reference: &S::Reference<'a>) -> Result<Import> {
        let reader = BufReader::new(self.scheme_handler.open_seekable(&reference)?);
        let gltf = Gltf::from_reader(reader)?;
        self.import_impl(gltf)
    }

    /// TODO
    pub fn import_slice(&self, slice: impl AsRef<[u8]>) -> Result<Import> {
        let gltf = Gltf::from_slice(slice.as_ref())?;
        self.import_impl(gltf)
    }

    /// TODO
    pub fn import_path(&self, path: impl AsRef<Path>) -> Result<Import> {
        let gltf = Gltf::from_reader(fs::File::open(path)?)?;
        self.import_impl(gltf)
    }

    fn import_impl(&self, Gltf { document, blob }: Gltf) -> Result<Import> {
        let buffer_data = self.import_buffers(&document, blob)?;
        let image_data = self.import_images(&document, &buffer_data)?;
        let import = (document, buffer_data, image_data);
        Ok(import)
    }

    fn read_to_end<'b>(&'b self, scheme: &S::Reference<'b>) -> Result<Vec<u8>> {
        // TODO: lifetime is all fucky
        let mut data = Vec::new();
        self.scheme_handler
            .open(scheme)?
            .read_to_end(&mut data)
            .map_err(Error::Io)?;
        Ok(data)
    }
}

// ---

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
        let importer = Importer::with_base(base);
        Self::import_from_source_and_blob(&importer, source, blob)
    }

    /// TODO
    pub fn import_from_source_and_blob<'a, S>(
        importer: &Importer<S>,
        source: buffer::Source<'_>,
        blob: &mut Option<Vec<u8>>,
    ) -> Result<Self>
    where
        S: ResourceResolver<'a>,
    {
        let mut data = match source {
            buffer::Source::Uri(uri) => importer.read_to_end(&S::parse(uri)),
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
    Importer::with_base(base).import_buffers(document, blob)
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
        let importer = Importer::with_base(base);
        Self::import_from_source(&importer, source, buffer_data)
    }

    /// TODO
    pub fn import_from_source<'a, S>(
        importer: &Importer<S>,
        source: image::Source<'_>,
        buffer_data: &[buffer::Data],
    ) -> Result<Self>
    where
        S: ResourceResolver<'a>,
    {
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
                let scheme = S::parse(uri);
                let encoded_image = importer.read_to_end(&scheme)?;

                // This is a bit of a hack to match the old behavior.
                let mime_type = scheme
                    .media_type()
                    .or(mime_type)
                    .or(scheme.extension().and_then(|e| match e {
                        "png" => Some("image/png"),
                        "jpg" | "jpeg" => Some("image/jpeg"),
                        #[cfg(feature = "EXT_texture_webp")]
                        "webp" => Some("image/webp"),
                        _ => return None,
                    }));

                let encoded_format = choose_format(mime_type, &encoded_image)?;
                (Cow::from(encoded_image), encoded_format)
            }
            image::Source::View { view, mime_type } => {
                let parent_buffer_data = &buffer_data[view.buffer().index()].0;
                let begin = view.offset();
                let end = begin + view.length();
                let encoded_image = &parent_buffer_data[begin..end];
                let encoded_format = choose_format(Some(mime_type), &encoded_image)?;
                (Cow::from(encoded_image), encoded_format)
            }
        };

        let decoded_image =
            image_crate::load_from_memory_with_format(&*encoded_image, encoded_format)?;
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
    Importer::with_base(base).import_images(document, buffer_data)
}

fn import_path(path: &Path) -> Result<Import> {
    let reference = DefaultScheme::File(path);
    let importer = Importer::<DefaultReferenceResolver>::with_base_from(&reference);
    importer.import_reference(&reference)
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
    Importer::<DefaultReferenceResolver>::with_base(None).import_slice(slice.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_uri_with_invalid_percent_encoding_is_rejected() {
        // `%FF%FE` decodes to bytes that are not valid UTF-8. The relative
        // branch must not panic on the decode error; instead it falls back
        // to `Scheme::Unsupported`, which `Scheme::read` reports as
        // `Error::UnsupportedScheme`.
        assert!(matches!(
            DefaultScheme::parse("%FF%FE"),
            DefaultScheme::Unsupported
        ));
        assert!(matches!(
            DefaultReferenceResolver::default().open(&DefaultScheme::parse("%FF%FE")),
            Err(Error::UnsupportedScheme)
        ));
    }
}
