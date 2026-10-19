//! Typed wrappers over [`MoodleClient::call`] for the functions used today.

use serde::Deserialize;
use serde_json::Value;

use crate::client::{MoodleClient, list_param};
use crate::error::{Error, Result};

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

#[derive(Debug, Deserialize)]
pub struct Page {
    pub id: u64,
    pub coursemodule: u64,
    pub course: u64,
    pub name: String,
    #[serde(default)]
    pub intro: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub timemodified: Option<u64>,
}

#[derive(Deserialize)]
struct PagesResponse {
    pages: Vec<Page>,
}

#[derive(Debug, Deserialize)]
pub struct Attachment {
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
}

#[derive(Debug, Deserialize)]
pub struct Assignment {
    pub id: u64,
    pub cmid: u64,
    pub course: u64,
    pub name: String,
    #[serde(default)]
    pub duedate: Option<u64>,
    #[serde(default)]
    pub allowsubmissionsfromdate: Option<u64>,
    #[serde(default)]
    pub intro: String,
    /// Files handed out with the task; they have no `type` field, unlike
    /// [`Content`], so they get their own struct.
    #[serde(default)]
    pub introattachments: Vec<Attachment>,
}

#[derive(Deserialize)]
struct AssignmentsResponse {
    courses: Vec<AssignmentCourse>,
}

#[derive(Deserialize)]
struct AssignmentCourse {
    #[serde(default)]
    assignments: Vec<Assignment>,
}

/// What the user has done on one assignment, as the todo list shows it.
#[derive(Debug, Default)]
pub struct SubmissionStatus {
    /// `new` | `draft` | `submitted` | …; `None` when nothing was ever started.
    pub status: Option<String>,
    pub submitted_at: Option<u64>,
    /// Moodle's display string, e.g. `85.00 / 100.00`.
    pub grade: Option<String>,
}

#[derive(Default, Deserialize)]
struct RawStatus {
    #[serde(default)]
    lastattempt: Option<RawAttempt>,
    #[serde(default)]
    feedback: Option<RawFeedback>,
}

#[derive(Default, Deserialize)]
struct RawAttempt {
    #[serde(default)]
    submission: Option<RawSubmission>,
    #[serde(default)]
    teamsubmission: Option<RawSubmission>,
}

#[derive(Deserialize)]
struct RawSubmission {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    timemodified: Option<u64>,
}

#[derive(Default, Deserialize)]
struct RawFeedback {
    #[serde(default)]
    gradefordisplay: Option<String>,
}

#[derive(Deserialize)]
struct Warning {
    #[serde(default)]
    warningcode: String,
    #[serde(default)]
    message: String,
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

    /// Pages of the given courses; the reader looks one up by `coursemodule`.
    pub async fn pages(&self, courseids: &[u64]) -> Result<Vec<Page>> {
        let ids: Vec<String> = courseids.iter().map(u64::to_string).collect();
        let owned = list_param("courseids", &ids);
        let params: Vec<(&str, String)> = owned
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        let value = self.call("mod_page_get_pages_by_courses", &params).await?;
        let response: PagesResponse = serde_json::from_value(value)?;
        Ok(response.pages)
    }

    /// Assignments of the given courses, flattened in response order.
    pub async fn assignments(&self, courseids: &[u64]) -> Result<Vec<Assignment>> {
        let ids: Vec<String> = courseids.iter().map(u64::to_string).collect();
        let owned = list_param("courseids", &ids);
        let params: Vec<(&str, String)> = owned
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        let value = self.call("mod_assign_get_assignments", &params).await?;
        let response: AssignmentsResponse = serde_json::from_value(value)?;
        Ok(flatten_assignments(response))
    }

    pub async fn submission_status(&self, assignid: u64) -> Result<SubmissionStatus> {
        let params = [("assignid", assignid.to_string())];
        let value = self
            .call("mod_assign_get_submission_status", &params)
            .await?;
        let raw: RawStatus = serde_json::from_value(value)?;
        Ok(pick_status(raw))
    }

    /// Attaches the draft area to the assignment's file submission (a draft, not yet submitted).
    pub async fn save_submission(&self, assignid: u64, itemid: u64) -> Result<()> {
        let params = [
            ("assignmentid", assignid.to_string()),
            ("plugindata[files_filemanager]", itemid.to_string()),
        ];
        let value = self.call("mod_assign_save_submission", &params).await?;
        check_warnings(value)
    }

    /// Final, usually irreversible "submit for grading".
    pub async fn submit_for_grading(&self, assignid: u64) -> Result<()> {
        let params = [
            ("assignmentid", assignid.to_string()),
            ("acceptsubmissionstatement", "1".to_string()),
        ];
        let value = self.call("mod_assign_submit_for_grading", &params).await?;
        check_warnings(value)
    }
}

fn flatten_assignments(response: AssignmentsResponse) -> Vec<Assignment> {
    let mut assignments: Vec<Assignment> = response
        .courses
        .into_iter()
        .flat_map(|course| course.assignments)
        .collect();
    // Moodle stores "no due date" as 0, which would otherwise sort first.
    for assignment in &mut assignments {
        if assignment.duedate == Some(0) {
            assignment.duedate = None;
        }
    }
    assignments
}

/// Group assignments report under `teamsubmission` instead of `submission`.
fn pick_status(raw: RawStatus) -> SubmissionStatus {
    let attempt = raw.lastattempt.unwrap_or_default();
    let submission = attempt.submission.or(attempt.teamsubmission);
    let grade = raw
        .feedback
        .and_then(|feedback| feedback.gradefordisplay)
        .filter(|grade| !grade.is_empty());
    let Some(submission) = submission else {
        return SubmissionStatus {
            grade,
            ..SubmissionStatus::default()
        };
    };
    SubmissionStatus {
        status: submission.status,
        submitted_at: submission.timemodified,
        grade,
    }
}

/// The save/submit functions answer with a warnings array instead of an
/// exception, so an empty array is the only success.
fn check_warnings(value: Value) -> Result<()> {
    let warnings: Vec<Warning> = serde_json::from_value(value)?;
    let Some(warning) = warnings.into_iter().next() else {
        return Ok(());
    };
    Err(Error::Moodle {
        code: warning.warningcode,
        message: warning.message,
    })
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

    #[test]
    fn deserializes_pages_response() {
        let json = r#"{"pages": [
            {"id": 12, "coursemodule": 9912, "course": 4821, "name": "Syllabus",
             "intro": "<p>Read me</p>", "content": "<h1>Welcome</h1>", "contentformat": 1,
             "revision": 3, "timemodified": 1700000000},
            {"id": 13, "coursemodule": 9913, "course": 4821, "name": "Grading",
             "content": "<p>Weights</p>", "displayoptions": "a:1:{}"}
        ], "warnings": []}"#;

        let response: PagesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.pages.len(), 2);
        let syllabus = &response.pages[0];
        assert_eq!(syllabus.coursemodule, 9912);
        assert_eq!(syllabus.name, "Syllabus");
        assert_eq!(syllabus.content, "<h1>Welcome</h1>");
        assert_eq!(syllabus.timemodified, Some(1700000000));
        let grading = &response.pages[1];
        assert_eq!(grading.coursemodule, 9913);
        assert_eq!(grading.name, "Grading");
        assert_eq!(grading.content, "<p>Weights</p>");
        assert_eq!(grading.intro, "");
        assert_eq!(grading.timemodified, None);
    }
    #[test]
    fn assignments_flatten_courses_and_drop_zero_duedate() {
        let json = r#"{"courses": [
            {"id": 7, "fullname": "Algebra", "assignments": [
                {"id": 55, "cmid": 9912, "course": 7, "name": "Homework 5",
                 "duedate": 1700000000, "allowsubmissionsfromdate": 0,
                 "intro": "<p>Solve</p>", "grade": 100,
                 "introattachments": [{"filename": "hw5.pdf", "filepath": "/", "filesize": 1234,
                   "fileurl": "https://moodle.test/webservice/pluginfile.php/1/mod_assign/intro/hw5.pdf",
                   "timemodified": 1699000000, "mimetype": "application/pdf"}]}
            ]},
            {"id": 8, "fullname": "Analysis", "assignments": [
                {"id": 56, "cmid": 9913, "course": 8, "name": "Essay", "duedate": 0}
            ]}
        ], "warnings": []}"#;

        let assignments = flatten_assignments(serde_json::from_str(json).unwrap());

        assert_eq!(assignments.len(), 2);
        let homework = &assignments[0];
        assert_eq!(homework.id, 55);
        assert_eq!(homework.cmid, 9912);
        assert_eq!(homework.course, 7);
        assert_eq!(homework.duedate, Some(1700000000));
        assert_eq!(homework.allowsubmissionsfromdate, Some(0));
        assert_eq!(homework.intro, "<p>Solve</p>");
        assert_eq!(homework.introattachments.len(), 1);
        let attachment = &homework.introattachments[0];
        assert_eq!(attachment.filename, "hw5.pdf");
        assert_eq!(attachment.filesize, Some(1234));
        assert_eq!(attachment.mimetype.as_deref(), Some("application/pdf"));
        assert!(
            attachment
                .fileurl
                .as_deref()
                .unwrap()
                .contains("pluginfile.php")
        );

        let essay = &assignments[1];
        assert_eq!(essay.course, 8);
        assert_eq!(essay.duedate, None);
        assert_eq!(essay.intro, "");
        assert!(essay.introattachments.is_empty());
    }

    #[test]
    fn pick_status_prefers_submission() {
        let json = r#"{"lastattempt": {
            "submission": {"id": 1, "status": "submitted", "timemodified": 1700000000},
            "teamsubmission": {"id": 2, "status": "draft", "timemodified": 1600000000}
        }, "feedback": {"gradefordisplay": "85.00 / 100.00"}}"#;
        let status = pick_status(serde_json::from_str(json).unwrap());
        assert_eq!(status.status.as_deref(), Some("submitted"));
        assert_eq!(status.submitted_at, Some(1700000000));
        assert_eq!(status.grade.as_deref(), Some("85.00 / 100.00"));
    }

    #[test]
    fn pick_status_falls_back_to_teamsubmission() {
        let json = r#"{"lastattempt": {
            "teamsubmission": {"id": 2, "status": "draft", "timemodified": 1600000000}
        }}"#;
        let status = pick_status(serde_json::from_str(json).unwrap());
        assert_eq!(status.status.as_deref(), Some("draft"));
        assert_eq!(status.submitted_at, Some(1600000000));
        assert_eq!(status.grade, None);
    }

    #[test]
    fn pick_status_handles_missing_parts() {
        let status = pick_status(serde_json::from_str("{}").unwrap());
        assert_eq!(status.status, None);
        assert_eq!(status.submitted_at, None);
        assert_eq!(status.grade, None);
    }

    #[test]
    fn pick_status_treats_empty_grade_as_none() {
        let json = r#"{"lastattempt": {"submission": {"status": "new"}},
            "feedback": {"gradefordisplay": ""}}"#;
        let status = pick_status(serde_json::from_str(json).unwrap());
        assert_eq!(status.status.as_deref(), Some("new"));
        assert_eq!(status.grade, None);
    }

    #[test]
    fn check_warnings_accepts_empty_array() {
        assert!(check_warnings(serde_json::json!([])).is_ok());
    }

    #[test]
    fn check_warnings_reports_first_warning() {
        let value = serde_json::json!([{
            "item": "Homework 5", "itemid": 1,
            "warningcode": "couldnotsavesubmission",
            "message": "Could not save submission."
        }]);
        match check_warnings(value) {
            Err(Error::Moodle { code, message }) => {
                assert_eq!(code, "couldnotsavesubmission");
                assert_eq!(message, "Could not save submission.");
            }
            other => panic!("expected Moodle error, got {other:?}"),
        }
    }
}
