#![allow(unused_mut)]
#![allow(unused_imports)]
#![warn(missing_docs)]

//! Parses `gl.xml` and then prints out Rust source that can bind to it.

use std::collections::HashMap;

use magnesium::{XmlElement::*, *};

fn revert_xml_encoding(text: String) -> String {
  let mut out = String::with_capacity(text.as_bytes().len());
  let mut chars = text.chars();
  while let Some(c) = chars.next() {
    if c != '&' {
      out.push(c);
    } else {
      match chars.next().unwrap() {
        'l' => {
          assert_eq!(chars.next().unwrap(), 't');
          assert_eq!(chars.next().unwrap(), ';');
          out.push('<');
        }
        'g' => {
          assert_eq!(chars.next().unwrap(), 't');
          assert_eq!(chars.next().unwrap(), ';');
          out.push('>');
        }
        'a' => {
          assert_eq!(chars.next().unwrap(), 'm');
          assert_eq!(chars.next().unwrap(), 'p');
          assert_eq!(chars.next().unwrap(), ';');
          out.push('&');
        }
        other => panic!("{}", other),
      }
    }
  }
  out
}

fn eat_to_comment_close<'s>(iter: &mut impl Iterator<Item = XmlElement<'s>>) {
  loop {
    match iter.next().unwrap() {
      EndTag { name: "comment" } => return,
      _ => continue,
    }
  }
}

fn eat_to_groups_close<'s>(iter: &mut impl Iterator<Item = XmlElement<'s>>) {
  loop {
    match iter.next().unwrap() {
      EndTag { name: "groups" } => return,
      _ => continue,
    }
  }
}

fn grab_out_name_text<'s>(
  iter: &mut impl Iterator<Item = XmlElement<'s>>,
) -> &'s str {
  let t = match iter.next().unwrap() {
    Text(t) => t,
    unknown => panic!("grab_out_name_text err:{:?}", unknown),
  };
  assert!(matches!(iter.next().unwrap(), EndTag { name: "name" }));
  t
}

fn grab_out_ptype_text<'s>(
  iter: &mut impl Iterator<Item = XmlElement<'s>>,
) -> &'s str {
  let t = match iter.next().unwrap() {
    Text(t) => t,
    unknown => panic!("grab_out_ptype_text err:{:?}", unknown),
  };
  assert!(matches!(iter.next().unwrap(), EndTag { name: "ptype" }));
  t
}

fn main() {
  let gl_xml =
    std::fs::read_to_string("target/gl.xml").expect("Couldn't read gl.xml");
  let mut iter = &mut ElementIterator::new(&gl_xml)
    .filter_map(skip_comments)
    .filter_map(skip_empty_text_elements);
  assert!(matches!(
    iter.next().unwrap(),
    StartTag { name: "registry", attrs: "" }
  ));
  let registry = GlRegistry::from_iter(iter);
  //println!("{:#?}", registry);
  for gl_enum in registry.gl_enums.iter() {
    println!("{}", GlEnumDisplayer { gl_enum, api: ApiGroup::Gl });
  }
}

/// Holds all the info accumulated from `gl.xml`.
#[derive(Debug, Default)]
pub struct GlRegistry {
  /// The special types we need to support.
  pub gl_types: Vec<GlType>,
  /// The special constant names we need defined.
  pub gl_enums: Vec<GlEnum>,
  /// The functions you'll be able to call.
  pub gl_commands: Vec<GlCommand>,
  /// The various API profiles you might target.
  pub gl_features: Vec<GlFeature>,
  /// The vendor extensions that you might wish to also try using.
  pub gl_extensions: Vec<GlExtension>,
}
impl GlRegistry {
  /// Build a `GlRegistry` from the XML iterator.
  pub fn from_iter<'s>(
    iter: &mut impl Iterator<Item = XmlElement<'s>>,
  ) -> Self {
    let mut registry = Self::default();
    loop {
      match iter.next().unwrap() {
        EndTag { name: "registry" } => return registry,
        StartTag { name: "comment", attrs: "" } => eat_to_comment_close(iter),
        StartTag { name: "groups", attrs: "" } => eat_to_groups_close(iter),
        StartTag { name: "types", attrs: "" } => loop {
          match iter.next().unwrap() {
            EndTag { name: "types" } => break,
            StartTag { name: "type", attrs } => {
              if let Some(t) = GlType::try_from_iter_and_attrs(iter, attrs) {
                registry.gl_types.push(t)
              }
            }
            unknown => panic!("unexpected 'type' tag content:{:?}", unknown),
          }
        },
        StartTag { name: "enums", attrs } => {
          gather_enum_entries_to(&mut registry.gl_enums, iter, attrs)
        }
        EmptyTag { name: "enums", attrs: _ } => {
          // Note(Lokathor): An empty enums tag is just like a start/end pair
          // except we define no enum entries, so we naturally just skip it.
        }
        StartTag { name: "commands", attrs: r#"namespace="GL""# } => loop {
          match iter.next().unwrap() {
            EndTag { name: "commands" } => break,
            StartTag { name: "command", attrs } => registry
              .gl_commands
              .push(GlCommand::from_iter_and_attrs(iter, attrs)),
            unknown => panic!("unknown 'commands' content:{:?}", unknown),
          }
        },
        StartTag { name: "feature", attrs } => {
          registry.gl_features.push(GlFeature::from_iter_and_attrs(iter, attrs))
        }
        StartTag { name: "extensions", attrs: "" } => loop {
          match iter.next().unwrap() {
            EndTag { name: "extensions" } => break,
            StartTag { name: "extension", attrs } => registry
              .gl_extensions
              .push(GlExtension::from_iter_and_attrs(iter, attrs)),
            EmptyTag { name: "extension", attrs } => {
              let mut extension = GlExtension::default();
              for TagAttribute { key, value } in
                TagAttributeIterator::new(attrs)
              {
                match key {
                  "name" => extension.name.push_str(value),
                  "supported" => extension.supported.push_str(value),
                  unknown => panic!("unknown: {:?}", unknown),
                }
              }
              registry.gl_extensions.push(extension);
            }
            unknown => panic!("{:?}", unknown),
          }
        },
        unknown => panic!("GlRegistry::from_iter:{:?}", unknown),
      }
    }
  }
}

/// Some sort of additional type we need to declare.
#[derive(Debug)]
pub enum GlType {
  /// A type alias for an existing type.
  Typedef(String),
  /// A new struct type.
  Struct(String),
  /// A type definition with conditional compilation in it.
  IfDef(String),
}
impl core::fmt::Display for GlType {
  fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
    match self {
      GlType::Typedef(s) => {
        assert!(s.as_bytes().last().unwrap() == &b';');
        let mut words_iter = s[..s.as_bytes().len() - 1].split_whitespace();
        assert_eq!(words_iter.next().unwrap(), "typedef");
        let mut new = words_iter.next_back().unwrap();
        let old: &'static str = match words_iter.next().unwrap() {
          "unsigned" => match words_iter.next().unwrap() {
            "int" => "c_uint",
            "char" => "c_uchar",
            "short" => "c_ushort",
            unknown => panic!("unknown unsigned:{}", unknown),
          },
          "void" => match words_iter.next() {
            None => "c_void",
            Some("*") => "*mut c_void",
            Some("(*") => match s.as_str() {
              "typedef void (* GLDEBUGPROC)(GLenum source,GLenum type,GLuint id,GLenum severity,GLsizei length,const GLchar *message,const void *userParam);" => {
                new = "GLDEBUGPROC";
                r#"Option<unsafe extern "system" fn(source: GLenum, gltype: GLenum, id: GLuint, severity: GLenum, length: GLsizei, message: *const GLchar, userParam: *mut c_void)>"#
              },
              "typedef void (* GLDEBUGPROCARB)(GLenum source,GLenum type,GLuint id,GLenum severity,GLsizei length,const GLchar *message,const void *userParam);" => {
                new = "GLDEBUGPROCARB";
                r#"Option<extern "system" fn(source: GLenum, gltype: GLenum, id: GLuint, severity: GLenum, length: GLsizei, message: *const GLchar, userParam: *mut c_void)>"#
              },
              "typedef void (* GLDEBUGPROCKHR)(GLenum source,GLenum type,GLuint id,GLenum severity,GLsizei length,const GLchar *message,const void *userParam);" => {
                new = "GLDEBUGPROCKHR";
                r#"Option<extern "system" fn(source: GLenum, gltype: GLenum, id: GLuint, severity: GLenum, length: GLsizei, message: *const GLchar, userParam: *mut c_void)>"#},
              "typedef void (* GLDEBUGPROCAMD)(GLuint id,GLenum category,GLenum severity,GLsizei length,const GLchar *message,void *userParam);" => {
                new = "GLDEBUGPROCAMD";
                r#"Option<extern "system" fn(id: GLuint, category: GLenum, severity: GLenum, length: GLsizei, message: *const GLchar, userParam: *mut c_void)>"#
              },
              "typedef void (* GLVULKANPROCNV)(void);" => {
                new = "GLVULKANPROCNV"; r#"Option<extern "system" fn()>"#
              },
              unknown => panic!("unknown fn ptr:{:?}", unknown),
            },
            unknown => panic!("unknown void:{:?}", unknown),
          },
          "struct" => match words_iter.next().unwrap() {
            "__GLsync" => {
              write!(f, "pub struct __GLsync{{ _priv: u8 }}")?;
              assert_eq!(words_iter.next().unwrap(), "*");
              "*mut GLsync"
            }
            unknown => panic!("unknown struct:{}", unknown),
          },
          "khronos_int8_t" => "i8",
          "khronos_uint8_t" => "u8",
          "khronos_int16_t" => "i16",
          "khronos_uint16_t" => "u16",
          "khronos_int32_t" => "i32",
          "khronos_uint32_t" => "u32",
          "khronos_int64_t" => "i64",
          "khronos_uint64_t" => "u64",
          "khronos_float_t" => "c_float",
          "khronos_intptr_t" => "isize",
          "khronos_ssize_t" => "isize",
          "GLintptr" => "GLintptr",
          "double" => "c_double",
          "int" => "c_int",
          "char" => "c_char",
          unknown => panic!("unknown:{}", unknown),
        };
        write!(f, "pub type {new} = {old};", new = new, old = old)
      }
      GlType::Struct(s) => {
        let mut words_iter = s[..s.as_bytes().len() - 1].split_whitespace();
        assert_eq!(words_iter.next().unwrap(), "struct");
        let name = words_iter.next().unwrap();
        write!(f, "pub struct {name}{{ _priv: u8 }}", name = name)
      }
      GlType::IfDef(s) => {
        assert_eq!(s, "#ifdef __APPLE__\r\ntypedef void *GLhandleARB;\r\n#else\r\ntypedef unsigned int GLhandleARB;\r\n#endif");
        write!(
          f,
          r#"#[cfg(any(target_os="macos", target_os="ios"))]pub type GLhandleARB = *mut c_void;#[cfg(not(any(target_os="macos", target_os="ios")))]pub type GLhandleARB = c_uint;"#
        )
      }
    }
  }
}
impl GlType {
  fn try_from_iter_and_attrs<'s>(
    iter: &mut impl Iterator<Item = XmlElement<'s>>,
    _attrs: &str,
  ) -> Option<Self> {
    //let attrs = hashmap_from_attrs(attrs);
    //println!("== Attrs: {:?}", attrs);
    let mut out = String::new();
    loop {
      match iter.next().unwrap() {
        EndTag { name: "type" } => break,
        StartTag { name: "name", attrs: "" } => {
          if !out.is_empty() {
            out.push(' ');
          }
          out.push_str(grab_out_name_text(iter))
        }
        Text(t) => out.push_str(t.trim()),
        EmptyTag { name: "apientry", attrs: "" } => (),
        unknown => panic!("unknown: {:?}", unknown),
      }
    }
    out = revert_xml_encoding(out);
    if out.starts_with("#include") {
      None
    } else if out.starts_with("typedef") {
      Some(GlType::Typedef(out))
    } else if out.starts_with("struct") {
      Some(GlType::Struct(out))
    } else if out.starts_with("#ifdef") {
      Some(GlType::IfDef(out))
    } else {
      panic!("unknown GlType variant: {}", out);
    }
  }
}

fn gather_enum_entries_to<'s>(
  list: &mut Vec<GlEnum>,
  iter: &mut impl Iterator<Item = XmlElement<'s>>,
  attrs: &str,
) {
  use magnesium::TagAttribute;
  let mut is_bitmask = false;
  for TagAttribute { key, value } in TagAttributeIterator::new(attrs) {
    match key {
      "namespace" => assert_eq!(value, "GL"),
      "group" | "comment" | "vendor" | "start" | "end" => (),
      "type" if value == "bitmask" => is_bitmask = true,
      unknown => panic!("unknown enum attr: {:?}", unknown),
    }
  }
  loop {
    match iter.next().unwrap() {
      EndTag { name: "enums" } => break,
      EmptyTag { name: "unused", attrs: _ } => (),
      EmptyTag { name: "enum", attrs } => {
        list.push(GlEnum::from_attrs(attrs, is_bitmask));
      }
      unknown => panic!("unknown: {:?}", unknown),
    }
  }
}

/// A constant we need to declare.
#[derive(Debug)]
pub struct GlEnum {
  /// The name
  pub name: String,
  /// The value.
  ///
  /// We keep it in string form because we want the final declaration within
  /// the source to be the same as we saw within gl.xml when possible.
  pub value: String,
  /// Some enums are within enum groups, which helps document possible values
  /// that can go to various function arguments.
  pub group: Option<String>,
  /// This enum entry is an alias for some other enum.
  pub alias_of: Option<String>,
  /// `GL_ACTIVE_PROGRAM_EXT` has different values depending on the API group.
  ///
  /// All other enums are the same between both OGL and GLES.
  pub api: Option<ApiGroup>,
  /// If this is set then we should define the const as `GLbitmask` (and then
  /// it would support bitwise ops) instead of `GLenum`
  pub is_bitmask: bool,
}
impl GlEnum {
  fn from_attrs(attrs: &str, is_bitmask: bool) -> Self {
    let mut name = String::new();
    let mut the_value = String::new();
    let mut group = None;
    let mut alias_of = None;
    let mut api = None;
    for TagAttribute { key, value } in TagAttributeIterator::new(attrs) {
      match key {
        "name" => name.push_str(value),
        "value" => the_value.push_str(value),
        "group" => group = Some(String::from(value)),
        "alias" => alias_of = Some(String::from(value)),
        "api" => api = Some(ApiGroup::from(value)),
        "comment" => (),
        "type" => (),
        unknown => panic!("unknown enum attr: {:?}", unknown),
      }
    }
    let value = the_value;
    assert!(!name.is_empty());
    assert!(!value.is_empty());
    GlEnum { name, value, group, alias_of, api, is_bitmask }
  }
}

/// Tags a `GlEnum` we're about to print as being for an `ApiGroup`.
#[derive(Debug)]
pub struct GlEnumDisplayer<'e> {
  /// The `GlEnum` we want to print.
  pub gl_enum: &'e GlEnum,
  /// The `ApiGroup` we're targeting with this print out.
  pub api: ApiGroup,
}
impl<'e> core::fmt::Display for GlEnumDisplayer<'e> {
  fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
    if let Some(a) = self.gl_enum.api {
      if self.api != a {
        return Ok(());
      }
    }
    let name = &self.gl_enum.name;
    let ty = if self.gl_enum.is_bitmask {
      "GLbitmask"
    } else if self.gl_enum.value == "0xFFFFFFFFFFFFFFFF" {
      "u64"
    } else {
      "GLenum"
    };
    let val = if self.gl_enum.value.starts_with("-") {
      format!("{} as GLenum", self.gl_enum.value)
    } else {
      self.gl_enum.value.clone()
    };
    let mut doc = format!(
      "`{name}: {ty} = {value_text}`",
      ty = ty,
      name = name,
      value_text = self.gl_enum.value,
    );
    if let Some(g) = self.gl_enum.group.as_ref() {
      doc.push_str(&format!(
        "\n* **Group{}:** ",
        if g.split(',').count() > 1 { "s" } else { "" }
      ));
      for (i, group) in g.split(',').enumerate() {
        if i != 0 {
          doc.push_str(", ");
        }
        doc.push_str(group);
      }
    }
    if let Some(a) = self.gl_enum.alias_of.as_ref() {
      doc.push_str("\n* **Alias Of:** `");
      doc.push_str(a);
      doc.push('`');
    }
    //
    write!(
      f,
      "#[doc = \"{doc}\"]\npub const {name}: {ty} = {val};",
      name = name,
      ty = ty,
      val = val,
      doc = doc
    )
  }
}

/// A GL function we have to bind to.
#[derive(Debug, Default)]
pub struct GlCommand {
  name: String,
  proto: String,
  proto_group: Option<String>,
  params: Vec<GlCommandParam>,
  glx_attrs: Option<String>,
  alias_of: Option<String>,
  /// "call this instead if you want to pass via pointer"
  vec_equivalent: Option<String>,
}
impl GlCommand {
  fn from_iter_and_attrs<'s>(
    iter: &mut impl Iterator<Item = XmlElement<'s>>,
    attrs: &str,
  ) -> Self {
    for TagAttribute { key, value: _ } in TagAttributeIterator::new(attrs) {
      match key {
        "comment" => (),
        unknown => panic!("unknown: {:?}", unknown),
      }
    }
    let mut command = GlCommand::default();
    loop {
      match iter.next().unwrap() {
        EndTag { name: "command" } => break,
        StartTag { name: "proto", attrs } => {
          if !attrs.is_empty() {
            for TagAttribute { key, value } in TagAttributeIterator::new(attrs)
            {
              match key {
                "group" => command.proto_group = Some(String::from(value)),
                unknown => panic!("unknown proto attr: {:?}", unknown),
              }
            }
          }
          loop {
            match iter.next().unwrap() {
              EndTag { name: "proto" } => break,
              Text(t) => command.proto.push_str(t),
              StartTag { name: "name", attrs: "" } => {
                let n = grab_out_name_text(iter);
                command.name.push_str(n);
                command.proto.push_str(n);
              }
              StartTag { name: "ptype", attrs: "" } => {
                let n = grab_out_ptype_text(iter);
                command.proto.push_str(n);
              }
              unknown => panic!("unknown: {:?}", unknown),
            }
          }
        }
        StartTag { name: "param", attrs } => {
          command.params.push(GlCommandParam::from_iter_and_attrs(iter, attrs))
        }
        EmptyTag { name: "glx", attrs } => {
          command.glx_attrs = Some(String::from(attrs));
        }
        EmptyTag { name: "alias", attrs } => {
          for TagAttribute { key, value } in TagAttributeIterator::new(attrs) {
            match key {
              "name" => command.alias_of = Some(String::from(value)),
              unknown => panic!("unknown: {:?}", unknown),
            }
          }
        }
        EmptyTag { name: "vecequiv", attrs } => {
          for TagAttribute { key, value } in TagAttributeIterator::new(attrs) {
            match key {
              "name" => command.vec_equivalent = Some(String::from(value)),
              unknown => panic!("unknown: {:?}", unknown),
            }
          }
        }
        unknown => panic!("unknown command content:{:?}", unknown),
      }
    }
    command
  }
}

/// An argument to a GL function.
#[derive(Debug, Default)]
pub struct GlCommandParam {
  text: String,
  group: Option<String>,
  len: Option<String>,
}
impl GlCommandParam {
  fn from_iter_and_attrs<'s>(
    iter: &mut impl Iterator<Item = XmlElement<'s>>,
    attrs: &str,
  ) -> Self {
    let mut text = String::new();
    let mut group = None;
    let mut len = None;
    for TagAttribute { key, value } in TagAttributeIterator::new(attrs) {
      match key {
        "group" => group = Some(String::from(value)),
        "len" => len = Some(String::from(value)),
        unknown => panic!("unknown: {:?}", unknown),
      }
    }
    loop {
      match iter.next().unwrap() {
        EndTag { name: "param" } => break,
        StartTag { name: "ptype", attrs: "" } => {
          text.push_str(grab_out_ptype_text(iter))
        }
        StartTag { name: "name", attrs: "" } => {
          text.push(' ');
          text.push_str(grab_out_name_text(iter))
        }
        Text(t) => text.push_str(t),
        unknown => panic!("unknown: {:?}", unknown),
      }
    }
    Self { text, group, len }
  }
}

/// A given GL API you can target.
#[derive(Debug, Default)]
pub struct GlFeature {
  /// What API group this feature is part of.
  pub api: ApiGroup,
  /// The name of the feature.
  pub name: String,
  /// The version number of the feature.
  pub number: String,
  /// New requirements compared to the previous feature in the same API group.
  pub required: Vec<GlRequirement>,
  /// Things to remove compared to the previous feature in the same API group.
  pub remove: Vec<GlRemoval>,
}
impl GlFeature {
  fn from_iter_and_attrs<'s>(
    iter: &mut impl Iterator<Item = XmlElement<'s>>,
    attrs: &str,
  ) -> Self {
    let mut feature = Self::default();
    for TagAttribute { key, value } in TagAttributeIterator::new(attrs) {
      match key {
        "api" => feature.api = ApiGroup::from(value),
        "name" => feature.name.push_str(value),
        "number" => feature.number.push_str(value),
        unknown => panic!("unknown: {:?}", unknown),
      }
    }
    loop {
      match iter.next().unwrap() {
        EndTag { name: "feature" } => return feature,
        StartTag { name: "require", attrs } => {
          let mut profile = None;
          let mut api = None;
          for TagAttribute { key, value } in TagAttributeIterator::new(attrs) {
            match key {
              "comment" => (),
              "profile" => profile = Some(String::from(value)),
              unknown => panic!("unknown: {:?}", unknown),
            }
          }
          loop {
            match iter.next().unwrap() {
              EndTag { name: "require" } => break,
              EmptyTag { name: "type", attrs } => {
                for TagAttribute { key, value } in
                  TagAttributeIterator::new(attrs)
                {
                  match key {
                    "name" => feature.required.push(GlRequirement {
                      profile: profile.clone(),
                      api: api.clone(),
                      adjustment: ReqRem::Type(String::from(value)),
                    }),
                    "comment" => (),
                    unknown => panic!("unknown: {:?}", unknown),
                  }
                }
              }
              EmptyTag { name: "enum", attrs } => {
                for TagAttribute { key, value } in
                  TagAttributeIterator::new(attrs)
                {
                  match key {
                    "name" => feature.required.push(GlRequirement {
                      profile: profile.clone(),
                      api: api.clone(),
                      adjustment: ReqRem::Enum(String::from(value)),
                    }),
                    "comment" => (),
                    unknown => panic!("unknown: {:?}", unknown),
                  }
                }
              }
              EmptyTag { name: "command", attrs } => {
                for TagAttribute { key, value } in
                  TagAttributeIterator::new(attrs)
                {
                  match key {
                    "name" => feature.required.push(GlRequirement {
                      profile: profile.clone(),
                      api: api.clone(),
                      adjustment: ReqRem::Command(String::from(value)),
                    }),
                    "comment" => (),
                    unknown => panic!("unknown: {:?}", unknown),
                  }
                }
              }
              unknown => panic!("unknown: {:?}", unknown),
            }
          }
        }
        EmptyTag { name: "require", attrs: _ } => (),
        StartTag { name: "remove", attrs } => {
          let mut profile = None;
          for TagAttribute { key, value } in TagAttributeIterator::new(attrs) {
            match key {
              "comment" => (),
              "profile" => profile = Some(String::from(value)),
              unknown => panic!("unknown: {:?}", unknown),
            }
          }
          loop {
            match iter.next().unwrap() {
              EndTag { name: "remove" } => break,
              EmptyTag { name: "type", attrs } => {
                for TagAttribute { key, value } in
                  TagAttributeIterator::new(attrs)
                {
                  match key {
                    "name" => feature.remove.push(GlRemoval {
                      profile: profile.clone(),
                      adjustment: ReqRem::Type(String::from(value)),
                    }),
                    "comment" => (),
                    unknown => panic!("unknown: {:?}", unknown),
                  }
                }
              }
              EmptyTag { name: "enum", attrs } => {
                for TagAttribute { key, value } in
                  TagAttributeIterator::new(attrs)
                {
                  match key {
                    "name" => feature.remove.push(GlRemoval {
                      profile: profile.clone(),
                      adjustment: ReqRem::Enum(String::from(value)),
                    }),
                    "comment" => (),
                    unknown => panic!("unknown: {:?}", unknown),
                  }
                }
              }
              EmptyTag { name: "command", attrs } => {
                for TagAttribute { key, value } in
                  TagAttributeIterator::new(attrs)
                {
                  match key {
                    "name" => feature.remove.push(GlRemoval {
                      profile: profile.clone(),
                      adjustment: ReqRem::Command(String::from(value)),
                    }),
                    "comment" => (),
                    unknown => panic!("unknown: {:?}", unknown),
                  }
                }
              }
              unknown => panic!("{:?}", unknown),
            }
          }
        }
        unknown => panic!("unknown 'feature' content:{:?}", unknown),
      }
    }
  }
}

/// Something that's new to a given API level.
///
/// These stack as you advance through the API levels.
#[derive(Debug)]
pub struct GlRequirement {
  /// Some requirements are limited to a specific profile.
  pub profile: Option<String>,
  /// Some requirements only apply to a given API group.
  pub api: Option<ApiGroup>,
  /// The requirement.
  pub adjustment: ReqRem,
}

/// Something to remove compared to the previous API level.
#[derive(Debug)]
pub struct GlRemoval {
  profile: Option<String>,
  adjustment: ReqRem,
}

/// Tags a requirement or removal as being a Type / Enum / Command.
#[derive(Debug)]
pub enum ReqRem {
  /// A required type.
  Type(String),
  /// A required enum.
  Enum(String),
  /// A required function.
  Command(String),
}

/// A vendor-specific API extension you might want to use.
#[derive(Debug, Default)]
pub struct GlExtension {
  /// The extension's name.
  pub name: String,
  /// `|` separated list of groups that can potentially support this extension.
  pub supported: String,
  /// Requirements if we want to generated bindings that include this
  /// extension.
  pub required: Vec<GlRequirement>,
}
impl GlExtension {
  fn from_iter_and_attrs<'s>(
    iter: &mut impl Iterator<Item = XmlElement<'s>>,
    attrs: &str,
  ) -> Self {
    let mut extension = Self::default();
    for TagAttribute { key, value } in TagAttributeIterator::new(attrs) {
      match key {
        "name" => extension.name.push_str(value),
        "supported" => extension.supported.push_str(value),
        "comment" => (),
        unknown => panic!("unknown: {:?}", unknown),
      }
    }
    loop {
      match iter.next().unwrap() {
        EndTag { name: "extension" } => return extension,
        StartTag { name: "require", attrs } => {
          let mut profile = None;
          let mut api = None;
          for TagAttribute { key, value } in TagAttributeIterator::new(attrs) {
            match key {
              "comment" => (),
              "profile" => profile = Some(String::from(value)),
              "api" => api = Some(ApiGroup::from(value)),
              unknown => panic!("unknown: {:?}", unknown),
            }
          }
          loop {
            match iter.next().unwrap() {
              EndTag { name: "require" } => break,
              EmptyTag { name: "type", attrs } => {
                for TagAttribute { key, value } in
                  TagAttributeIterator::new(attrs)
                {
                  match key {
                    "name" => extension.required.push(GlRequirement {
                      profile: profile.clone(),
                      api: api.clone(),
                      adjustment: ReqRem::Type(String::from(value)),
                    }),
                    "comment" => (),
                    unknown => panic!("unknown: {:?}", unknown),
                  }
                }
              }
              EmptyTag { name: "enum", attrs } => {
                for TagAttribute { key, value } in
                  TagAttributeIterator::new(attrs)
                {
                  match key {
                    "name" => extension.required.push(GlRequirement {
                      profile: profile.clone(),
                      api: api.clone(),
                      adjustment: ReqRem::Enum(String::from(value)),
                    }),
                    "comment" => (),
                    unknown => panic!("unknown: {:?}", unknown),
                  }
                }
              }
              EmptyTag { name: "command", attrs } => {
                for TagAttribute { key, value } in
                  TagAttributeIterator::new(attrs)
                {
                  match key {
                    "name" => extension.required.push(GlRequirement {
                      profile: profile.clone(),
                      api: api.clone(),
                      adjustment: ReqRem::Command(String::from(value)),
                    }),
                    "comment" => (),
                    unknown => panic!("unknown: {:?}", unknown),
                  }
                }
              }
              unknown => panic!("unknown: {:?}", unknown),
            }
          }
        }
        unknown => panic!("unknown 'feature' content:{:?}", unknown),
      }
    }
  }
}

/// The broad API groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiGroup {
  /// OpenGL
  Gl,
  /// OpenGL ES 1
  Gles1,
  /// OpenGL ES 2 (and also 3)
  Gles2,
  /// OpenGL SC
  Glsc2,
}
impl Default for ApiGroup {
  fn default() -> Self {
    ApiGroup::Gl
  }
}
impl From<&str> for ApiGroup {
  fn from(s: &str) -> Self {
    match s {
      "gl" => ApiGroup::Gl,
      "gles1" => ApiGroup::Gles1,
      "gles2" => ApiGroup::Gles2,
      "glsc2" => ApiGroup::Glsc2,
      _ => panic!("illegal:{}", s),
    }
  }
}
