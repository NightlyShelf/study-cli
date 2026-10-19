//! Typed wrappers over [`MoodleClient::call`] for the functions used today.

use serde::Deserialize;

use crate::client::MoodleClient;
use crate::error::Result;

#[derive(Debug, Deserialize)]
pub struct SiteInfo {
    pub userid: u64,
    pub sitename: String,
    pub fullname: String,
    pub username: String,
    #[serde(default)]
    pub functions: Vec<FunctionInfo>,
}

#[derive(Debug, Deserialize)]
pub struct FunctionInfo {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct Course {
    pub id: u64,
    pub shortname: String,
    pub fullname: String,
    #[serde(default)]
    pub lastaccess: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct Section {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub uservisible: Option<bool>,
    #[serde(default)]
    pub modules: Vec<Module>,
}

#[derive(Debug, Deserialize)]
pub struct Module {
    pub id: u64,
    pub name: String,
    pub modname: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub uservisible: Option<bool>,
    #[serde(default)]
    pub contents: Vec<Content>,
    #[serde(default)]
    pub contentsinfo: Option<ContentsInfo>,
}

#[derive(Debug, Deserialize)]
pub struct Content {
    /// `file` or `url`.
    #[serde(rename = "type")]
    pub kind: String,
    pub filename: String,
    #[serde(default)]
    pub filepath: Option<String>,
    #[serde(default)]
    pub filesize: Option<u64>,
    #[serde(default)]
    pub fileurl: Option<String>,
    #[serde(default)]
    pub mimetype: Option<String>,
    #[serde(default)]
    pub timemodified: Option<u64>,
    /// 1 on a resource's main file; its support files (an HTML page's
    /// `.js`/`.css`/images) have 0.
    #[serde(default)]
    pub sortorder: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct ContentsInfo {
    #[serde(default)]
    pub lastmodified: Option<u64>,
}

impl MoodleClient {
    pub async fn site_info(&self) -> Result<SiteInfo> {
        let value = self.call("core_webservice_get_site_info", &[]).await?;
        Ok(serde_json::from_value(value)?)
    }

    pub async fn user_courses(&self, userid: u64) -> Result<Vec<Course>> {
        let params = [("userid", userid.to_string())];
        let value = self.call("core_enrol_get_users_courses", &params).await?;
        Ok(serde_json::from_value(value)?)
    }

    pub async fn course_contents(&self, courseid: u64) -> Result<Vec<Section>> {
        let params = [("courseid", courseid.to_string())];
        let value = self.call("core_course_get_contents", &params).await?;
        Ok(serde_json::from_value(value)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_course_contents() {
        let json = r#"[{
            "id": 10,
            "name": "Week 1",
            "uservisible": true,
            "modules": [
                {
                    "id": 101,
                    "name": "Lecture slides",
                    "modname": "resource",
                    "url": "https://moodle.test/mod/resource/view.php?id=101",
                    "uservisible": true,
                    "contentsinfo": {"filescount": 1, "lastmodified": 1700000000},
                    "contents": [{
                        "type": "file",
                        "filename": "slides.pdf",
                        "filepath": "/",
                        "filesize": 2048,
                        "fileurl": "https://moodle.test/webservice/pluginfile.php/5/mod_resource/content/1/slides.pdf?forcedownload=1",
                        "mimetype": "application/pdf",
                        "timemodified": 1700000000,
                        "sortorder": 1
                    }]
                },
                {
                    "id": 102,
                    "name": "Course site",
                    "modname": "url",
                    "url": "https://moodle.test/mod/url/view.php?id=102",
                    "uservisible": false
                }
            ]
        }]"#;

        let sections: Vec<Section> = serde_json::from_str(json).unwrap();
        assert_eq!(sections.len(), 1);
        let section = &sections[0];
        assert_eq!(section.id, 10);
        assert_eq!(section.name, "Week 1");
        assert_eq!(section.uservisible, Some(true));
        assert_eq!(section.modules.len(), 2);

        let resource = &section.modules[0];
        assert_eq!(resource.modname, "resource");
        assert_eq!(resource.contents.len(), 1);
        assert_eq!(
            resource.contentsinfo.as_ref().unwrap().lastmodified,
            Some(1700000000)
        );
        let file = &resource.contents[0];
        assert_eq!(file.kind, "file");
        assert_eq!(file.filename, "slides.pdf");
        assert_eq!(file.filesize, Some(2048));
        assert_eq!(file.mimetype.as_deref(), Some("application/pdf"));
        assert_eq!(file.sortorder, Some(1));
        assert!(file.fileurl.as_deref().unwrap().contains("pluginfile.php"));

        let link = &section.modules[1];
        assert_eq!(link.modname, "url");
        assert_eq!(link.uservisible, Some(false));
        assert!(link.contents.is_empty());
        assert!(link.contentsinfo.is_none());
    }

    #[test]
    fn deserializes_courses_ignoring_unknown_fields() {
        let json = r#"[
            {"id": 12, "shortname": "ALG", "fullname": "Algebra", "lastaccess": 1700000000,
             "enrolledusercount": 42, "format": "topics", "hidden": false},
            {"id": 13, "shortname": "ANA", "fullname": "Analysis"}
        ]"#;

        let courses: Vec<Course> = serde_json::from_str(json).unwrap();
        assert_eq!(courses.len(), 2);
        assert_eq!(courses[0].id, 12);
        assert_eq!(courses[0].shortname, "ALG");
        assert_eq!(courses[0].lastaccess, Some(1700000000));
        assert_eq!(courses[1].fullname, "Analysis");
        assert_eq!(courses[1].lastaccess, None);
    }

    #[test]
    fn deserializes_site_info() {
        let json = r#"{"userid": 7, "sitename": "Test Moodle", "fullname": "Ada Lovelace",
            "username": "ada", "functions": [{"name": "core_course_get_contents", "version": "2024"}],
            "release": "4.3"}"#;
        let info: SiteInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.userid, 7);
        assert_eq!(info.functions[0].name, "core_course_get_contents");
    }
}
