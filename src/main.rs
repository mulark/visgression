use core::ops::AddAssign;
use megabase_index_incrementer::*;
use std::path::Path;
use std::path::PathBuf;
use std::convert::TryFrom;
use charts::{Chart, ScaleLinear,
	ScaleBand, VerticalBarView, BarLabelPosition, AxisPosition};
use rusqlite::Connection;
use rusqlite::NO_PARAMS;
use std::collections::HashMap;
use std::collections::BTreeMap;

//const LAST_MAJOR_VERSION: FactorioVersion = FactorioVersion::new(0,18,45???);

const LAST_MINOR_VERSIONS: [FactorioVersion; 2] = [
	FactorioVersion::new(0,16,51),
	FactorioVersion::new(0,17,79),
];

const START_GRAPH_FV: FactorioVersion = FactorioVersion::new(0,17,66);
const END_GRAPH_FV: FactorioVersion = FactorioVersion::new(0,18,45);

/// Generates a list of all FactorioVersions between START_GRAPH_FV and END_GRAPH_FV
/// Advances major/minor versions when necessary.
fn iter_factorio_versions() -> Vec<FactorioVersion> {
	let mut all_ord_fv = Vec::new();
	let mut cur_fv = START_GRAPH_FV;
	loop {
		all_ord_fv.push(cur_fv);
		if LAST_MINOR_VERSIONS.contains(&cur_fv) {
			cur_fv.minor += 1;
			cur_fv.patch = 0;
		} else {
			cur_fv.patch += 1;
		}
		if cur_fv > END_GRAPH_FV {
			break;
		}
	}
	all_ord_fv
}

/// A collection of averaged data for a given factorio version
#[allow(non_snake_case)]
#[derive(Debug, Clone, PartialEq, Copy, Default)]
struct AvgData {
	wholeUpdate: f64,
	circuitNetworkUpdate: f64,
	transportLinesUpdate: f64,
	fluidsUpdate: f64,
	entityUpdate: f64,
	electricNetworkUpdate: f64,
	logisticManagerUpdate: f64,
	trains: f64,
	trainPathFinder: f64,
}

impl AddAssign for AvgData {
	fn add_assign(&mut self, other: AvgData) {
		self.wholeUpdate += other.wholeUpdate;
		self.circuitNetworkUpdate += other.circuitNetworkUpdate;
		self.transportLinesUpdate += other.transportLinesUpdate;
		self.fluidsUpdate += other.fluidsUpdate;
		self.entityUpdate += other.entityUpdate;
		self.electricNetworkUpdate += other.electricNetworkUpdate;
		self.logisticManagerUpdate += other.logisticManagerUpdate;
		self.trains += other.trains;
		self.trainPathFinder += other.trainPathFinder;
	}
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, PartialOrd, Ord)]
struct MapInfo {
	map_name: String,
	sha256: String,
}

fn query_db<P: AsRef<Path>>(db_loc: P) -> Result<BTreeMap<MapInfo, BTreeMap<FactorioVersion, AvgData>>, Box<dyn std::error::Error>> {
	if !db_loc.as_ref().exists() {
		panic!("Could not find a suitable regression test database. Try running factorio-benchmark-helper with the --regression-test flag, was passed {:?}", db_loc.as_ref());
	}
	let db = Connection::open(db_loc)?;
    // Define chart related sizes.
	let mut stmt = db.prepare(
r#"select factorio_version,
avg(wholeUpdate)/1000000.0 as wholeUpdate,
avg(circuitNetworkUpdate)/1000000.0 as circuitNetworkUpdate,
avg(transportLinesUpdate)/1000000.0 as transportLinesUpdate,
avg(fluidsUpdate)/1000000.0 as fluidsUpdate,
avg(entityUpdate)/1000000.0 as entityUpdate,
avg(electricNetworkUpdate)/1000000.0 as electricNetworkUpdate,
avg(logisticManagerUpdate)/1000000.0 as logisticMangerUpdate,
avg(trains)/1000000.0 as trains,
avg(trainPathFinder)/1000000.0 as trainPathFinder,
sha256,
map_name
from verbose join regression_test_instance
on verbose.instance_ID = regression_test_instance.ID
join regression_scenario
on regression_scenario.ID = regression_test_instance.scenario_ID
group by instance_id
order by scenario_ID, factorio_version;"#)?;

	let data = stmt.query_map(NO_PARAMS, |row| {
		let fv = FactorioVersion::try_from(row.get::<_, String>(0)?.as_ref()).unwrap();
		assert!(fv <= END_GRAPH_FV, "Factorio version {} exceeds END_GRAPH_FV", fv.to_string());
		assert!(fv >= START_GRAPH_FV, "Factorio version {} precedes START_GRAPH_FV", fv.to_string());
		Ok((
			fv,
			AvgData {
				wholeUpdate: row.get(1)?,
				circuitNetworkUpdate: row.get(2)?,
				transportLinesUpdate: row.get(3)?,
				fluidsUpdate: row.get(4)?,
				entityUpdate: row.get(5)?,
				electricNetworkUpdate: row.get(6)?,
				logisticManagerUpdate: row.get(7)?,
				trains: row.get(8)?,
				trainPathFinder: row.get(9)?,

			},
			MapInfo {
				sha256: row.get(10)?,
				map_name: row.get(11)?,
			}
	))
	})?;
	let mut maps = BTreeMap::new();

	for mapped_row in data {
		let (fv, data, map_info) = mapped_row?;
		let entry = maps.entry(map_info).or_insert_with(BTreeMap::new);
		entry.insert(fv, data);
	}

	Ok(maps)
}

/// Aggregate-transforms a set of Maps that have been tested in various Factorio
/// versions into a collection of distinct version map chains.
/// ex:
/// ```
/// factorio_version|count(scenario_id)
/// 0.17.79|4
/// 0.18.0|5
/// 0.18.17|6
/// 0.18.18|6
/// 0.18.19|6
/// ```
/// will return the average of the 4 maps present in 0.17.79 for all 5 example
/// versions, the average of the 5 maps present in 0.18.0 for for 4 versions,
/// and the average of 6 maps present for the remaining 3 versions.
fn aggregate_maps(maps: &BTreeMap<MapInfo, BTreeMap<FactorioVersion, AvgData>>)
-> BTreeMap<FactorioVersion, (Vec<MapInfo>, BTreeMap<FactorioVersion, AvgData>)> {
	//FV, ct of FV
	let mut aggregation_versions = BTreeMap::new();
	for versions_data in maps.values() {
		for fv in versions_data.keys() {
			*aggregation_versions.entry(*fv).or_insert(0) += 1;
		}
	}
	let mut prev_seen_ct = 0;

	// The FactorioVersions at which a new scenario was included.
	let mut checkpoints = Vec::new();
	for (fv, ct) in aggregation_versions {
		if prev_seen_ct < ct {
			prev_seen_ct = ct;
			checkpoints.push(fv);
		}
	}

	let mut checkpoint_buckets: BTreeMap<FactorioVersion, BTreeMap<FactorioVersion, AvgData>> = BTreeMap::new();
	let mut info_buckets: BTreeMap<FactorioVersion, Vec<MapInfo>> = BTreeMap::new();
	for checkpoint_fv in checkpoints {
		let mut maps_per_checkpoint_inner_fv = HashMap::new();
		for (info, versions_data) in maps {
			if !versions_data.contains_key(&checkpoint_fv) {
				continue;
			}
			info_buckets.entry(checkpoint_fv).or_default().push(info.clone());
			for (fv, avg) in versions_data {
				if fv < &checkpoint_fv {
					continue;
				}
				// Save the count of maps seen for this specific version
				*maps_per_checkpoint_inner_fv.entry(fv).or_insert(0) += 1;

				*checkpoint_buckets
					.entry(checkpoint_fv).or_default()
					.entry(*fv).or_default() += *avg;
			}
		}
		// Transform to avg
		for (fv, dat) in checkpoint_buckets.get_mut(&checkpoint_fv).unwrap() {
			*dat = AvgData {
				wholeUpdate: dat.wholeUpdate / *maps_per_checkpoint_inner_fv.get(fv).unwrap() as f64,
				circuitNetworkUpdate: dat.circuitNetworkUpdate / *maps_per_checkpoint_inner_fv.get(fv).unwrap() as f64,
				transportLinesUpdate: dat.transportLinesUpdate / *maps_per_checkpoint_inner_fv.get(fv).unwrap() as f64,
				fluidsUpdate: dat.fluidsUpdate / *maps_per_checkpoint_inner_fv.get(fv).unwrap() as f64,
				entityUpdate: dat.entityUpdate / *maps_per_checkpoint_inner_fv.get(fv).unwrap() as f64,
				electricNetworkUpdate: dat.electricNetworkUpdate / *maps_per_checkpoint_inner_fv.get(fv).unwrap() as f64,
				logisticManagerUpdate: dat.logisticManagerUpdate / *maps_per_checkpoint_inner_fv.get(fv).unwrap() as f64,
				trains: dat.trains / *maps_per_checkpoint_inner_fv.get(fv).unwrap() as f64,
				trainPathFinder: dat.trainPathFinder / *maps_per_checkpoint_inner_fv.get(fv).unwrap() as f64,
			}
		}
	}
	let ret: BTreeMap<FactorioVersion, (Vec<MapInfo>, BTreeMap<FactorioVersion, AvgData>)>;
	ret = info_buckets.into_iter().zip(checkpoint_buckets).map(|((info_fv, info), (_checkpoint_fv, avg_data))| {
		(info_fv, (info, avg_data))
	}).collect();
	ret
}

fn gen_svg(collective_fv: Option<FactorioVersion>, map_infos: &[MapInfo], versions_data: &BTreeMap<FactorioVersion, AvgData>)
		-> Result<PathBuf, Box<dyn std::error::Error>> {
	let width = 1280;
    let height = 640;
    let (top, right, bottom, left) = (90, 200, 50, 60);
	let mut fvs = iter_factorio_versions();

	fvs.retain(|fv| versions_data.contains_key(&fv));
	let mut data_points = Vec::new();
	for (vers, data) in versions_data {
		data_points.push((vers, data.entityUpdate, "entityUpdate"));
		data_points.push((vers, data.circuitNetworkUpdate, "circuitNetworkUpdate"));
		data_points.push((vers, data.transportLinesUpdate, "transportLinesUpdate"));
		data_points.push((vers, data.fluidsUpdate, "fluidsUpdate"));
		data_points.push((vers, data.electricNetworkUpdate, "electricNetworkUpdate"));
		data_points.push((vers, data.logisticManagerUpdate, "logisticManagerUpdate"));
		data_points.push((vers, data.trains, "trains"));
		data_points.push((vers, data.trainPathFinder, "trainPathFinder"));
		data_points.push((vers,
			(data.wholeUpdate - data.circuitNetworkUpdate -
				data.transportLinesUpdate - data.fluidsUpdate -
				data.entityUpdate - data.electricNetworkUpdate -
				data.logisticManagerUpdate - data.trains - data.trainPathFinder
			), "other"));
	}
	let data_points = data_points.iter().map(|(a, dta, b)| (a.to_string(), *dta as f32, b.to_string())).collect();

	let x = ScaleBand::new()
		.set_domain(fvs.into_iter().map(|x| x.to_string()).collect())
		.set_range(vec![0, width - left - right]);

	let y = ScaleLinear::new()
		.set_domain(vec![0_f32, 20.])
		.set_range(vec![height - top - bottom, 0]);

	let line_view = VerticalBarView::new()
		.set_x_scale(&x)
		.set_y_scale(&y)
		.set_label_position(BarLabelPosition::Center)
		.set_label_rounding_precision(1)
		.load_data(&data_points).unwrap();
	let title;
	if let Some(fv) = collective_fv {
		title = "Maps beginning in ".to_owned() + &fv.to_string();
	} else {
		title = map_infos[0].map_name.clone();
	}
	let savefile = if let Some(fv) = collective_fv {
		PathBuf::from(fv.to_string() + ".svg")
	} else {
		PathBuf::from(title.to_owned() + ".svg")
	};
	Chart::new()
		.set_width(width)
		.set_height(height)
		.set_margins(top, right, bottom, left)
		.add_title(title)
		.add_view(&line_view)
		.add_axis_bottom(&x)
		.add_axis_left(&y)
		.add_legend_at(AxisPosition::Right)
		.add_left_axis_label("Average update time (ms)")
		.add_bottom_axis_label("Factorio Version")
		.set_bottom_axis_tick_label_rotation(-30)
		.save(&savefile).unwrap();

	let s = std::fs::read_to_string(&savefile)?;
	let s = s.replace("sans-serif", "Bitstream Vera Sans Mono, monospace");
	std::fs::write(&savefile, s)?;
	Ok(savefile)
}

struct HtmlEmitter {
	svg: PathBuf,
	sel_list_name: String,
	ext_descr: Vec<(String, String)>,
}

/// Downloads and parses the technicalfactorio megabase index.
fn fetch_megabase_list() -> Result<Megabases, Box<dyn std::error::Error>> {
    let resp = ureq::get("https://raw.githubusercontent.com/technicalfactorio/\
        technicalfactorio/master/megabase_index_incrementer/megabases.json")
        .call();
    if resp.status() == 200 {
        let s = resp.into_string()?;
        Ok(serde_json::from_str(&s)?)
    } else {
        eprintln!("Could not download listing of megabases");
        std::process::exit(1);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
	let args = std::env::args();
	let args: Vec<_> = args.collect();
	if args.is_empty() || args.len() == 1 {
		eprintln!("Usage: cargo run -- $PATH_TO_REGRESSION_DB");
		eprintln!("cargo run -- --default to attempt to use the below path");
		eprintln!("Probably ~/.local/share/factorio-benchmark-helper/regression-testing/regression.db");
		std::process::exit(0);
	}
	let path = if args[args.len() - 1] == "--default" {
		let p = PathBuf::from(".local/share/factorio-benchmark-helper/regression-testing/regression.db");
		#[allow(deprecated)]
		std::env::home_dir().unwrap().join(p)
	} else {
		PathBuf::from(&args[args.len() - 1])
	};

	let megabases = fetch_megabase_list()?;
	let mut map_name_to_post_link = HashMap::new();
	for megabase in megabases.saves {
		map_name_to_post_link.insert(megabase.name, megabase.source_link);
	}

	// You should point this at the actual regression testing database
	// it probably lives in
	// ~/.local/share/factorio-benchmark-helper/regression-testing/regression.db
	let maps = query_db(path)?;

	let aggregation = aggregate_maps(&maps);
	let mut html_emitters = Vec::new();

	for (fv, (map_info, avg_data)) in aggregation {
		let svg = gen_svg(Some(fv), &map_info, &avg_data)?;

		let template = HtmlEmitter {
			svg,
			sel_list_name: "Maps beginning with ".to_owned() + &fv.to_string(),
			ext_descr: map_info.iter().map(|x| (map_name_to_post_link.get(&x.map_name).unwrap().to_string(), x.map_name.clone())).collect(),
		};
		html_emitters.push(template);
	}

	for data in maps {
		let svg = gen_svg(None, &[data.0.clone()], &data.1)?;
		let template = HtmlEmitter {
			svg,
			sel_list_name: data.0.map_name.replace(".zip",""),
			ext_descr: vec![(map_name_to_post_link.get(&data.0.map_name).unwrap().to_string(), data.0.map_name.clone())],
		};
		html_emitters.insert(0, template);
	}

	eprintln!("<select class=\"selections\">");
	for emitter in &html_emitters {
		eprintln!("    <option onclick = \"setSlide()\">{}</option>", emitter.sel_list_name);
	}
	eprintln!("</select>");
/*
<div class = "slides">
    <div class = "slide">
        <img src="images/0.17-0.18 Poobers Beautiful Base.zip.svg"/>
        <p>Some random content</p>
    </div>
*/
	eprintln!("<div class = \"slides\">");
	for emitter in &html_emitters {
		eprintln!("    <div class = \"slide\">");
		eprintln!("        <img src=\"images/{}\"/>", emitter.svg.to_str().unwrap());
		if !emitter.ext_descr.is_empty() {
			eprintln!("        <ul>");
			for (post_link, desc) in &emitter.ext_descr {
				eprintln!("            <li><a href=\"{}\">{}</a>", post_link, desc);
			}
			eprintln!("        </ul>");
		}

		eprintln!("    </div>");
	}
	eprintln!("</div>");
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn test_aggr() {
		let p = PathBuf::from(".local/share/factorio-benchmark-helper/regression-testing/regression.db");
		#[allow(deprecated)]
		let path = std::env::home_dir().unwrap().join(p);
		let maps = query_db(path).unwrap();

		aggregate_maps(&maps);
	}
}
