use ergo_lib::ergotree_ir::ergo_tree::ErgoTree;
use ergo_lib::ergotree_ir::serialization::SigmaSerializable;
use lazy_static::lazy_static;

const POOL_VALIDATOR_BYTES: &str =
    "19c0062904000400040204020404040404060406040804080404040204000400040204020601010400040a05000500\
    0404040204020e20fc3cdbfd1abc83f4a38ca3fb3dfe417a158b67d63e3c52137fdda4e66ad3956c040004020500040\
    2040204060500050005feffffffffffffffff010502050005000402050005000100d820d601b2a5730000d602db6308\
    7201d603db6308a7d604b27203730100d605e4c6a70410d606e4c6a70505d607e4c6a70605d608b27202730200d609b\
    27203730300d60ab27202730400d60bb27203730500d60cb27202730600d60db27203730700d60e8c720d01d60fb272\
    02730800d610b27203730900d6118c721001d6128c720b02d613998c720a027212d6148c720902d615b27205730a00d\
    6169a99a37215730bd617b27205730c00d6189d72167217d61995919e72167217730d9a7218730e7218d61ab2720573\
    0f00d61b7e721a05d61c9d7206721bd61d998c720c028c720d02d61e8c721002d61f998c720f02721ed6207310d1ede\
    dededed93b272027311007204ededed93e4c672010410720593e4c672010505720693e4c6720106057207928cc77201\
    018cc7a70193c27201c2a7ededed938c7208018c720901938c720a018c720b01938c720c01720e938c720f01721193b\
    172027312959172137313d802d6219c721399721ba273147e721905d622b2a5731500ededed929a997206721472079c\
    7e9995907219721a72199a721a7316731705721c937213f0721d937221f0721fedededed93cbc272227318938602720\
    e7213b2db6308722273190093860272117221b2db63087222731a00e6c67222040893e4c67222050e8c720401958f72\
    13731bededec929a997206721472079c7e9995907219721a72199a721a731c731d05721c92a39a9a72159c721a7217b\
    27205731e0093721df0721392721f95917219721a731f9c721d99721ba273207e721905d804d621e4c672010704d622\
    99721a7221d6237e722205d62499997321721e9c9972127322722395ed917224732391721f7324edededed907221997\
    2197325909972149c7223721c9a721c7207907ef0998c7208027214069a9d9c99997e7214069d9c7e7206067e722206\
    7e721a0672207e721f067e7224067220937213732693721d73277328";

const BUNDLE_VALIDATOR_BYTES: &str =
    "19a3041f040004000404040404000402060101040005000402040404020400040004020502040405020402040005fe\
    ffffffffffffffff010408050205000404040004060404040205fcffffffffffffffff010100d80cd601b2a5730000d\
    602db63087201d603e4c6a7050ed604b2a4730100d605db63087204d6068cb2720573020002d607998cb27202730300\
    027206d608e4c6a70408d609db6308a7d60ab27209730400d60bb27205730500d60c7306d1ed938cb27202730700017\
    203959372077308d80cd60db2a5e4e3000400d60eb2a5e4e3010400d60fb2e4c672040410730900d610c672010804d6\
    1199720f95e67210e47210e4c672010704d6128cb27209730a0001d613db6308720ed614b27209730b00d6158c72140\
    2d6167e721105d6179972159c72168c720a02d618b2db6308720d730c00eded93c2720dd07208edededed93e4c6720e\
    0408720893e4c6720e050e720393c2720ec2a795917211730dd801d619b27213730e00eded9386027212730fb272137\
    31000938c7219018c721401939972158c721902721793860272127311b2721373120093b27213731300720aed938c72\
    18018c720b01927e8c7218020699999d9c99997e8c720b02069d9c7ee4c672040505067e7211067e720f06720c7e721\
    7067e999973148cb27205731500029c9972067316721606720c720c958f7207731793b2db6308b2a473180073190086\
    029593b17209731a8cb27209731b00018cb27209731c0001731d731e";

const DEPOSIT_TEMPLATE_BYTES: &str =
    "d808d601b2a4730000d602db63087201d6037301d604b2a5730200d6057303d606c57201d607b2a5730400d6088cb2\
    db6308a773050002eb027306d1ededed938cb27202730700017203ed93c27204720593860272067308b2db630872047\
    30900ededededed93cbc27207730a93d0e4c672070408720593e4c67207050e72039386028cb27202730b00017208b2\
    db63087207730c009386028cb27202730d00019c72087e730e05b2db63087207730f0093860272067310b2db6308720\
    773110090b0ada5d90109639593c272097312c1720973137314d90109599a8c7209018c7209027315";

pub const REDEEM_VALIDATOR_BYTES: &str =
    "19ad020a040208cd02217daf90deb73bdf8b6709bb42093fdfaff6573fd47b630e2d3fdd4a8193a74d0e2001010101\
    010101010101010101010101010101010101010101010101010101010e2000000000000000000000000000000000000\
    0000000000000000000000000000005d00f04000e691005040004000e36100204a00b08cd0279be667ef9dcbbac55a0\
    6295ce870b07029bfcdb2dce28d959f2815b16f81798ea02d192a39a8cc7a701730073011001020402d19683030193a\
    38cc7b2a57300000193c2b2a57301007473027303830108cdeeac93b1a573040500050005a09c01d801d601b2a57300\
    00eb027301d1eded93c27201730293860273037304b2db6308720173050090b0ada5d90102639593c272027306c1720\
    273077308d90102599a8c7202018c7202027309";

const REDEEM_TEMPLATE_BYTES: &str =
    "d801d601b2a5730000eb027301d1eded93c27201730293860273037304b2db6308720173050090b0ada5d901026395\
    93c272027306c1720273077308d90102599a8c7202018c7202027309";

lazy_static! {
    pub static ref POOL_VALIDATOR: ErgoTree =
        ErgoTree::sigma_parse_bytes(&base16::decode(POOL_VALIDATOR_BYTES.as_bytes()).unwrap()).unwrap();
    pub static ref BUNDLE_VALIDATOR: ErgoTree =
        ErgoTree::sigma_parse_bytes(&base16::decode(BUNDLE_VALIDATOR_BYTES.as_bytes()).unwrap()).unwrap();
    pub static ref REDEEM_VALIDATOR: ErgoTree =
        ErgoTree::sigma_parse_bytes(&base16::decode(REDEEM_VALIDATOR_BYTES.as_bytes()).unwrap()).unwrap();
    pub static ref DEPOSIT_TEMPLATE: Vec<u8> = base16::decode(DEPOSIT_TEMPLATE_BYTES.as_bytes()).unwrap();
    pub static ref REDEEM_TEMPLATE: Vec<u8> = base16::decode(REDEEM_TEMPLATE_BYTES.as_bytes()).unwrap();
}
