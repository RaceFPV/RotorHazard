# ClusterNodeSet:  Manages a set of secondary nodes

import logging
import gevent
import json
import socketio
from monotonic import monotonic
import RHUtils
from RHRace import RaceStatus
from eventmanager import Evt
from util.RunningMedian import RunningMedian
from util.Averager import Averager
from util.SendAckQueue import SendAckQueue

logger = logging.getLogger(__name__)


class SecondaryNode:

    SPLIT_MODE = 'split'
    MIRROR_MODE = 'mirror'
    ACTION_MODE = 'action'

    LATENCY_AVG_SIZE = 30
    TIMEDIFF_MEDIAN_SIZE = 30
    TIMEDIFF_CORRECTION_THRESH_MS = 250  # correct split times if secondary clock more off than this

    def __init__(self, idVal, info, RaceContext, monotonic_to_epoch_millis, server_release_version):
        self.id = idVal
        self.info = info
        self._racecontext = RaceContext
        self.monotonic_to_epoch_millis = monotonic_to_epoch_millis
        self.server_release_version = server_release_version
        addr = info['address']
        if not '://' in addr:
            addr = 'http://' + addr
        self.address = addr
        modeStr = str(info.get('mode', SecondaryNode.SPLIT_MODE)).lower()
        if modeStr == SecondaryNode.MIRROR_MODE:
            self.isMirrorMode = True
            self.isActionMode = False
            self.isSplitMode = False
            self.secondaryModeStr = SecondaryNode.MIRROR_MODE
        elif modeStr == SecondaryNode.ACTION_MODE:
            self.isMirrorMode = False
            self.isActionMode = True
            self.isSplitMode = False
            self.secondaryModeStr = SecondaryNode.ACTION_MODE
            if 'event' not in self.info:  # set default event name if none given
                self.info['event'] = 'SecondaryActionTimer_{}'.format(self.id + 1)
        else:
            self.isMirrorMode = False
            self.isActionMode = False
            self.isSplitMode = True
            self.secondaryModeStr = SecondaryNode.SPLIT_MODE
        self.recEventsFlag = info.get('recEventsFlag', self.isMirrorMode)
        self.queryInterval = info.get('queryInterval', 0)
        if self.queryInterval <= 0:
            self.queryInterval = 10
        self.firstQueryInterval = 3 if self.queryInterval >= 3 else 1
        self.queryTimeout = info.get('timeout', 300)
        self.distance = float(info.get('distance', 0.0)) * 1000.0
        self.startConnectTime = 0
        self.lastContactTime = -1
        self.firstContactTime = 0
        self.lastCheckQueryTime = 0
        self.secsSinceDisconnect = 0
        self.freqsSentFlag = False
        self.numDisconnects = 0
        self.numDisconnsDuringRace = 0
        self.numContacts = 0
        self.latencyAveragerObj = Averager(self.LATENCY_AVG_SIZE)
        self.totalUpTimeSecs = 0
        self.totalDownTimeSecs = 0
        self.timeDiffMedianObj = RunningMedian(self.TIMEDIFF_MEDIAN_SIZE)
        self.timeDiffMedianMs = 0
        self.timeCorrectionMs = 0
        self.progStartEpoch = 0
        self.runningFlag = False
        self.parentNodeSet = None
        self.actionPassTimes = {}
        self.sio = socketio.Client(reconnection=False, request_timeout=1)
        self.sio.on('connect', self.on_connect)
        self.sio.on('disconnect', self.on_disconnect)
        self.sio.on('pass_record', self.on_pass_record)
        self.sio.on('check_secondary_response', self.on_check_secondary_response)
        self.sio.on('join_cluster_response', self.join_cluster_response)
        self.start_connection()

    def start_connection(self):
        self.startConnectTime = 0
        self.lastContactTime = -1
        self.firstContactTime = 0
        self.lastCheckQueryTime = 0
        self.secsSinceDisconnect = 0
        self.freqsSentFlag = False
        self.numDisconnects = 0
        self.numDisconnsDuringRace = 0
        self.numContacts = 0
        if len(self.latencyAveragerObj) > 0:
            self.latencyAveragerObj = Averager(self.LATENCY_AVG_SIZE)
        self.totalUpTimeSecs = 0
        self.totalDownTimeSecs = 0
        if (len(self.timeDiffMedianObj.sorted_) > 0):
            self.timeDiffMedianObj = RunningMedian(self.TIMEDIFF_MEDIAN_SIZE)
        self.timeDiffMedianMs = 0
        self.timeCorrectionMs = 0
        self.progStartEpoch = 0
        gevent.spawn(self.secondary_worker_thread)
        self.runningFlag = True

    def secondary_worker_thread(self):
        self.startConnectTime = monotonic()
        gevent.sleep(0.1)
        while self.runningFlag:
            try:
                gevent.sleep(1)
                if self.lastContactTime <= 0:  # if current status is not connected
                    oldSecsSinceDis = self.secsSinceDisconnect
                    self.secsSinceDisconnect = monotonic() - self.startConnectTime
                    if self.secsSinceDisconnect >= 1.0:  # if disconnect just happened then wait a second before reconnect
                        # if never connected then only retry if race not in progress
                        if self.numDisconnects > 0 or (self._racecontext.race.race_status != RaceStatus.STAGING and \
                                                        self._racecontext.race.race_status != RaceStatus.RACING):
                            # if first-ever attempt or was previously connected then show log msg
                            if oldSecsSinceDis == 0 or self.numDisconnects > 0:
                                logger.log((logging.INFO if self.secsSinceDisconnect <= self.queryTimeout else logging.DEBUG), \
                                           "Attempting to connect to secondary {0} at {1}...".format(self.id+1, self.address))
                            try:
                                self.sio.connect(self.address)
                            except socketio.exceptions.ConnectionError as ex:
                                if self.lastContactTime > 0:  # if current status is connected
                                    logger.info("Error connecting to secondary {0} at {1}: {2}".format(self.id+1, self.address, ex))
                                    if not self.sio.connected:  # if not connected then
                                        self.on_disconnect()    # invoke disconnect function to update status
                                else:
                                    err_msg = "Unable to connect to secondary {0} at {1}: {2}".format(self.id+1, self.address, ex)
                                    if monotonic() <= self.startConnectTime + self.queryTimeout:
                                        if self.numDisconnects > 0:  # if previously connected then always log failure
                                            logger.info(err_msg)
                                        elif oldSecsSinceDis == 0:   # if not previously connected then only log once
                                            err_msg += " (will continue attempts until timeout)"
                                            logger.info(err_msg)
                                    else:  # if beyond timeout period
                                        if self.numDisconnects > 0:  # if was previously connected then keep trying
                                            logger.debug(err_msg)    #  log at debug level and
                                            gevent.sleep(29)         #  increase delay between attempts
                                        else:
                                            logger.warning(err_msg)     # if never connected then give up
                                            logger.warning("Reached timeout; no longer trying to connect to secondary {0} at {1}".\
                                                        format(self.id+1, self.address))
                                            if self.runningFlag and self._racecontext.rhui.emit_cluster_connect_change:
                                                self._racecontext.rhui.emit_cluster_connect_change(False)  # play one disconnect tone
                                            self.runningFlag = False
                                            return  # exit worker thread
                else:  # if current status is connected
                    now_time = monotonic()
                    if not self.freqsSentFlag:
                        try:
                            self.freqsSentFlag = True
                            if (not self.isMirrorMode) and self._racecontext.race.profile:
                                logger.info("Sending node frequencies to secondary {0} at {1}".format(self.id+1, self.address))
                                for idx, freq in enumerate(json.loads(self._racecontext.race.profile.frequencies)["f"]):
                                    data = { 'node':idx, 'frequency':freq }
                                    self.emit('set_frequency', data)
                                    gevent.sleep(0.001)
                        except (KeyboardInterrupt, SystemExit): #pylint disable=try-except-raise
                            raise
                        except Exception as ex:
                            logger.error("Error sending node frequencies to secondary {0} at {1}: {2}".format(self.id+1, self.address, ex))
                    else:
                        try:
                            if self.sio.connected:
                                # send heartbeat-query every 'queryInterval' seconds, or that long since last contact
                                if (now_time > self.lastContactTime + self.queryInterval and \
                                            now_time > self.lastCheckQueryTime + self.queryInterval) or \
                                            (self.lastCheckQueryTime == 0 and \
                                             now_time > self.lastContactTime + self.firstQueryInterval):  # if first query do it sooner
                                    self.lastCheckQueryTime = now_time
                                    # timestamp not actually used by secondary, but send to make query and response symmetrical
                                    payload = {
                                        'timestamp': self.monotonic_to_epoch_millis(now_time) \
                                                         if self.monotonic_to_epoch_millis else 0
                                    }
                                    # don't update 'lastContactTime' value until response received
                                    self.sio.emit('check_secondary_query', payload)
                                # if there was no response to last query then disconnect (and reconnect next loop)
                                elif self.lastCheckQueryTime > self.lastContactTime:
                                    if self.lastCheckQueryTime - self.lastContactTime > 3.9:
                                        if len(self.timeDiffMedianObj.sorted_) > 0:
                                            logger.warning("Disconnecting after no response for 'check_secondary_query'" \
                                                     " received for secondary {0} at {1}".format(self.id+1, self.address))
                                            # calling 'disconnect()' will usually invoke 'on_disconnect()', but
                                            #  'disconnect()' can be slow to return, so force-update status if needed
                                            gevent.spawn(self.do_sio_disconnect)
                                            if self.wait_for_sio_disconnect(1.0):
                                                logger.info("Forcing 'disconnected' status for stuck connection on" \
                                                            " secondary {0} at {1}".format(self.id+1, self.address))
                                                self.on_disconnect()
                                        else:  # if never any responses then may be old server version on secondary timer
                                            logger.warning("No response for 'check_secondary_query'" \
                                                           " received for secondary {0} at {1} (may need upgrade)".\
                                                           format(self.id+1, self.address))
                                            self.lastCheckQueryTime = self.lastContactTime = now_time
                                    else:
                                        logger.debug("No response for 'check_secondary_query' received "\
                                                     "after {0:.1f} secs for secondary {1} at {2}".\
                                                     format(self.lastCheckQueryTime - self.lastContactTime, \
                                                            self.id+1, self.address))
                            else:
                                logger.info("Invoking 'on_disconnect()' fn for secondary {0} at {1}".\
                                            format(self.id+1, self.address))
                                self.on_disconnect()
                        except (KeyboardInterrupt, SystemExit): #pylint disable=try-except-raise
                            raise
                        except Exception as ex:
                            logger.error("Error sending check-query to secondary {0} at {1}: {2}".format(self.id+1, self.address, ex))
            except KeyboardInterrupt:
                logger.info("SecondaryNode worker thread terminated by keyboard interrupt")
                raise
            except SystemExit: #pylint disable=try-except-raise
                raise
            except Exception as ex:
                if type(ex) is ValueError and "connected" in str(ex):  # if error was because already connected
                    if self.lastContactTime <= 0:  # if current tracked status is not connected
                        logger.debug("Ignoring connect error from sio-already-connected on secondary {0}".\
                                     format(self.id+1))
                    else:
                        logger.info("Forcing 'disconnected' status after sio-already-connected error on" \
                                    " secondary {0}".format(self.id+1))
                        self.on_disconnect()
                else:
                    logger.exception("Exception in SecondaryNode worker thread for secondary {} (sio.conn={})".\
                                     format(self.id+1, self.sio.connected))
                    gevent.sleep(9)

    def emit(self, event, data = None):
        try:
            if self.lastContactTime > 0:
                self.sio.emit(event, data)
                self.lastContactTime = monotonic()
                self.numContacts += 1
            elif self.numDisconnects > 0:  # only warn if previously connected
                logger.warning("Unable to emit to disconnected secondary {0} at {1}, event='{2}'".\
                            format(self.id+1, self.address, event))
        except Exception:
            logger.exception("Error emitting to secondary {0} at {1}, event='{2}'".\
                            format(self.id+1, self.address, event))
            if self.sio.connected:
                logger.warning("Disconnecting after error emitting to secondary {0} at {1}".\
                            format(self.id+1, self.address))
                self.sio.disconnect()

    def on_connect(self):
        try:
            if self.lastContactTime <= 0:
                self.lastContactTime = monotonic()
                self.firstContactTime = self.lastContactTime
                if self.numDisconnects <= 0:
                    logger.info("Connected to secondary {0} at {1} (mode: {2})".format(\
                                        self.id+1, self.address, self.secondaryModeStr))
                else:
                    downSecs = int(round(self.lastContactTime - self.startConnectTime)) if self.startConnectTime > 0 else 0
                    logger.info("Reconnected to " + self.get_log_str(downSecs, False))
                    self.totalDownTimeSecs += downSecs
                payload = {
                    'mode': self.secondaryModeStr
                }
                self.emit('join_cluster_ex', payload)
                if (not self.isMirrorMode) and \
                        (self._racecontext.race.race_status == RaceStatus.STAGING or self._racecontext.race.race_status == RaceStatus.RACING):
                    self.emit('stage_race')  # if race in progress then make sure running on secondary
                if self.runningFlag and self._racecontext.rhui.emit_cluster_connect_change:
                    self._racecontext.rhui.emit_cluster_connect_change(True)
            else:
                self.lastContactTime = monotonic()
                logger.debug("Received extra 'on_connect' event for secondary {0} at {1}".format(self.id+1, self.address))
        except Exception:
            logger.exception("Error handling Cluster 'on_connect' for secondary {0} at {1}".\
                             format(self.id+1, self.address))

    def on_disconnect(self):
        try:
            if self.lastContactTime > 0:
                self.startConnectTime = monotonic()
                self.lastContactTime = -1
                self.numDisconnects += 1
                self.numDisconnsDuringRace += 1
                upSecs = int(round(self.startConnectTime - self.firstContactTime)) if self.firstContactTime > 0 else 0
                logger.warning("Disconnected from " + self.get_log_str(upSecs))
                self.totalUpTimeSecs += upSecs
                if self.runningFlag and self._racecontext.rhui.emit_cluster_connect_change:
                    self._racecontext.rhui.emit_cluster_connect_change(False)
            else:
                logger.debug("Received extra 'on_disconnect' event for secondary {0} at {1}".format(self.id+1, self.address))
        except Exception:
            logger.exception("Error handling Cluster 'on_disconnect' for secondary {0} at {1}".\
                             format(self.id+1, self.address))

    def do_sio_disconnect(self):
        try:
            if self.sio.connected:
                self.sio.disconnect()
                logger.debug("Returned from 'sio.disconnect()' call for secondary {0} at {1}".\
                             format(self.id+1, self.address))
        except Exception:
            logger.exception("Error calling 'sio.disconnect()' for secondary {0} at {1}".\
                             format(self.id+1, self.address))

    def wait_for_sio_disconnect(self, maxWaitSecs):
        dly = maxWaitSecs / 10
        cnt = 10 if dly > 0 else 0
        while True:
            if not self.sio.connected:
                return False
            if cnt <= 0:
                return True
            cnt -= 1
            gevent.sleep(dly)

    def get_log_str(self, timeSecs=None, upTimeFlag=True, stoppedRaceFlag=False):
        if timeSecs is None:
            timeSecs = int(round(monotonic() - self.firstContactTime)) if self.lastContactTime > 0 else 0
        totUpSecs = self.totalUpTimeSecs
        totDownSecs = self.totalDownTimeSecs
        if upTimeFlag:
            totUpSecs += timeSecs
            upDownStr = "upTime"
        else:
            totDownSecs += timeSecs
            upDownStr = "downTime"
        upDownTotal = totUpSecs + totDownSecs
        return "secondary {0} at {1} (latency: min={2} avg={3} max={4} last={5} ms, disconns={6}, contacts={7}, " \
               "timeDiff={8}ms, {9}={10}, totalUp={11}, totalDown={12}, avail={13:.1%}{14})".\
                    format(self.id+1, self.address, self.latencyAveragerObj.minVal, \
                           self.latencyAveragerObj.getIntAvgVal(), self.latencyAveragerObj.maxVal, \
                           self.latencyAveragerObj.lastVal, self.numDisconnects, self.numContacts, \
                           self.timeDiffMedianMs, upDownStr, timeSecs, totUpSecs, totDownSecs, \
                           (float(totUpSecs)/upDownTotal if upDownTotal > 0 else 0),
                           ((", numDisconnsDuringRace=" + str(self.numDisconnsDuringRace)) if \
                                    (self.numDisconnsDuringRace > 0 and \
                                     (stoppedRaceFlag or self._racecontext.race.race_status == RaceStatus.RACING)) else ""))

    def on_pass_record(self, data):
        try:
            now_secs = monotonic()
            self.lastContactTime = now_secs
            self.numContacts += 1
            node_index = data['node']

            if self._racecontext.race.race_status is RaceStatus.RACING:

                pilot_id = self._racecontext.rhdata.get_pilot_from_heatNode(self._racecontext.race.current_heat, node_index) 
                
                if pilot_id != RHUtils.PILOT_ID_NONE:

                    if not self.isActionMode:

                        # convert split timestamp (epoch ms sine 1970-01-01) to equivalent local 'monotonic' time value
                        split_ts = data['timestamp'] - self._racecontext.race.start_time_epoch_ms
        
                        act_laps_list = self._racecontext.race.get_active_laps()[node_index]
                        lap_count = max(0, len(act_laps_list) - 1)
                        split_id = self.id
        
                        # get timestamp for last lap pass (including lap 0)
                        if len(act_laps_list) > 0:
                            last_lap_ts = act_laps_list[-1]['lap_time_stamp']
                            lap_split = self._racecontext.rhdata.get_lapSplits_by_lap(node_index, lap_count)
        
                            if len(lap_split) <= 0: # first split for this lap
                                if split_id > 0:
                                    logger.info('Ignoring missing splits before {0} for node {1}'.format(split_id+1, node_index+1))
                                last_split_ts = last_lap_ts
                            else:
                                last_split_id = lap_split[-1].id 
                                if split_id > last_split_id:
                                    if split_id > last_split_id + 1:
                                        logger.info('Ignoring missing splits between {0} and {1} for node {2}'.format(last_split_id+1, split_id+1, node_index+1))
                                    last_split_ts = lap_split.split_time_stamp
                                else:
                                    logger.info('Ignoring out-of-order split {0} for node {1}'.format(split_id+1, node_index+1))
                                    last_split_ts = None
                        else:
                            logger.info('Ignoring split {0} before zero lap for node {1}'.format(split_id+1, node_index+1))
                            last_split_ts = None
        
                        if last_split_ts is not None:
        
                            # if secondary-timer clock was detected as not synchronized then apply correction
                            if self.timeCorrectionMs != 0:
                                split_ts -= self.timeCorrectionMs
        
                            split_time = split_ts - last_split_ts
                            split_speed = self.distance / float(split_time) if self.distance > 0.0 else None
                            split_time_str = RHUtils.time_format(split_time, self._racecontext.rhdata.get_option('timeFormat'))
                            logger.debug('Split pass record: Node {0}, lap {1}, split {2}, time={3}, speed={4}' \
                                .format(node_index+1, lap_count+1, split_id+1, split_time_str, \
                                ('{0:.2f}'.format(split_speed) if split_speed is not None else 'None')))
        
                            self._racecontext.rhdata.add_lapSplit({
                                'node_index': node_index,
                                'pilot_id': pilot_id,
                                'lap_id': lap_count,
                                'splid_id': split_id,
                                'split_time_stamp': split_ts,
                                'split_time': split_time,
                                'split_time_formatted': split_time_str,
                                'split_speed': split_speed
                            })
                            
                            self._racecontext.rhui.emit_split_pass_info(pilot_id, split_id, split_time)

                    else:  # Action mode
                        pilot = self._racecontext.rhdata.get_pilot(pilot_id)
                        minRepeatSecs = self.info.get('minRepeatSecs', 10)
                        if now_secs - self.actionPassTimes.get(node_index, 0) >= minRepeatSecs:
                            self.actionPassTimes[node_index] = now_secs
                            eventStr = self.info.get('event', 'runEffect')
                            effectStr = self.info.get('effect', None)
                            logger.info("Secondary 'action' timer pass record: Node {}, pilot_id {}, callsign {}, event='{}', effect={}".\
                                        format(node_index+1, pilot_id, pilot.callsign, eventStr, effectStr))
                            duration = int(self.info.get('toneDuration', 0))
                            if duration > 0:
                                frequency = int(self.info.get('toneFrequency', 0))
                                volume = int(self.info.get('toneVolume', 100))
                                toneType = self.info.get('toneType', 'square')
                                if frequency > 0 and volume > 0:
                                    self._racecontext.rhui.emit_play_beep_tone(duration, frequency, volume, toneType)
                            if effectStr and len(effectStr) > 0:
                                if self.parentNodeSet and self.parentNodeSet.Events:
                                    self.parentNodeSet.Events.trigger(eventStr, {'pilot_id': pilot.id})
                                else:
                                    logger.warning("Secondary 'action' timer pass record without 'parentNodeSet' configured")
                        else:
                            logger.info("Ignoring secondary 'action' timer pass record too soon after previous (limit={} secs): Node {}, pilot_id {}, callsign {}".\
                                        format(minRepeatSecs, node_index+1, pilot_id, pilot.callsign))
                
                else:
                    logger.info('Split pass record dismissed: Node: {0}, no pilot on node'.format(node_index+1))

            else:
                logger.info('Ignoring split {0} for node {1} because race not running'.format(self.id+1, node_index+1))

        except Exception:
            logger.exception("Error processing pass record from secondary {0} at {1}".format(self.id+1, self.address))

        try:
            # send message-ack back to secondary (but don't update 'lastContactTime' value)
            payload = {
                'messageType': 'pass_record',
                'messagePayload': data
            }
            self.sio.emit('cluster_message_ack', payload)
        except Exception:
            logger.exception("Error sending pass-record message acknowledgement to secondary {0} at {1}".\
                             format(self.id+1, self.address))

    def on_check_secondary_response(self, data):
        try:
            if self.lastContactTime > 0:
                nowTime = monotonic()
                self.lastContactTime = nowTime
                self.numContacts += 1
                transitTime = nowTime - self.lastCheckQueryTime if self.lastCheckQueryTime > 0 else 0
                if transitTime > 0:
                    self.latencyAveragerObj.addItem(int(round(transitTime * 1000)))
                    if data:
                        secondaryTimestamp = data.get('timestamp', 0)
                        if secondaryTimestamp:
                            # calculate local-time value midway between before and after network query
                            localTimestamp = self.monotonic_to_epoch_millis(\
                                             self.lastCheckQueryTime + transitTime/2) \
                                             if self.monotonic_to_epoch_millis else 0
                            # calculate clock-time difference in ms and add to running median
                            self.timeDiffMedianObj.insert(int(round(secondaryTimestamp - localTimestamp)))
                            self.timeDiffMedianMs = self.timeDiffMedianObj.median()
                            return
                    logger.debug("Received check_secondary_response with no timestamp from secondary {0} at {1}".\
                                 format(self.id+1, self.address))
            else:
                logger.debug("Received check_secondary_response while disconnected from secondary {0} at {1}".\
                             format(self.id+1, self.address))
        except Exception:
            logger.exception("Error processing check-response from secondary {0} at {1}".\
                             format(self.id+1, self.address))

    def join_cluster_response(self, data):
        try:
            infoStr = data.get('server_info')
            logger.debug("Server info from secondary {0} at {1}:  {2}".\
                         format(self.id+1, self.address, infoStr))
            infoDict = json.loads(infoStr)
            prgStrtEpchStr = infoDict.get('prog_start_epoch')
            newPrgStrtEpch = False
            try:
                prgStrtEpch = int(float(prgStrtEpchStr))
                if self.progStartEpoch == 0:
                    self.progStartEpoch = prgStrtEpch
                    newPrgStrtEpch = True
                    logger.debug("Initial 'prog_start_epoch' value for secondary {0}: {1}".\
                                format(self.id+1, prgStrtEpch))
                elif prgStrtEpch != self.progStartEpoch:
                    self.progStartEpoch = prgStrtEpch
                    newPrgStrtEpch = True
                    logger.info("New 'prog_start_epoch' value for secondary {0}: {1}; resetting 'timeDiff' median".\
                                format(self.id+1, prgStrtEpch))
                    self.timeDiffMedianObj = RunningMedian(self.TIMEDIFF_MEDIAN_SIZE)
            except ValueError as ex:
                logger.warning("Error parsing 'prog_start_epoch' value from secondary {0}: {1}".\
                            format(self.id+1, ex))
            # if first time connecting (or possible secondary restart) then check/warn about program version
            if newPrgStrtEpch or self.numDisconnects == 0:
                secondaryVerStr = infoDict.get('release_version')
                if secondaryVerStr:
                    if secondaryVerStr != self.server_release_version:
                        logger.warning("Different program version ('{0}') running on secondary {1} at {2}".\
                                    format(secondaryVerStr, self.id+1, self.address))
                else:
                    logger.warning("Unable to parse 'release_version' from secondary {0} at {1}".\
                                format(self.id+1, self.address))
        except Exception:
            logger.exception("Error processing join-cluster response from secondary {0} at {1}".\
                             format(self.id+1, self.address))
        try:
            # send message-ack back to secondary (but don't update 'lastContactTime' value)
            #  this tells secondary timer to expect future message-acks in response to 'pass_record' emits
            payload = { 'messageType': 'join_cluster_response' }
            self.sio.emit('cluster_message_ack', payload)
        except Exception:
            logger.exception("Error sending join-cluster message acknowledgement to secondary {0} at {1}".\
                             format(self.id+1, self.address))


class ClusterNodeSet:
    def __init__(self, Language, eventmanager):
        self._Language = Language
        self.secondaries = []
        self.splitSecondaries = []
        self.recEventsSecondaries = []
        self.Events = eventmanager
        self.eventActionsObj = None
        self.ClusterSendAckQueueObj = None

    def setEventActionsObj(self, eventActionsObj):
        self.eventActionsObj = eventActionsObj
        # add event-effects configured for action timers (if not already present)
        for secondary in self.splitSecondaries:
            if secondary.isActionMode:
                event = secondary.info.get('event')
                effect = secondary.info.get('effect', 'speak')
                if event and (not eventActionsObj.containsAction(event)) and str(effect).lower() != 'none':
                    logger.info("Adding event '{}' for secondary action-mode timer {} at {}".\
                                format(event, secondary.id+1, secondary.address))
                    eventActionsObj.addEventAction(event, effect, \
                                              secondary.info.get('text', ''))

    def emit_cluster_msg_to_primary(self, SOCKET_IO, messageType, messagePayload, waitForAckFlag=True):
        '''Emits cluster message to primary timer.'''
        if not self.ClusterSendAckQueueObj:
            self.ClusterSendAckQueueObj = SendAckQueue(20, SOCKET_IO, logger)
        self.ClusterSendAckQueueObj.put(messageType, messagePayload, waitForAckFlag)

    def emit_cluster_ack_to_primary(self, messageType, messagePayload):
        '''Emits cluster message-acknowledge to primary timer.'''
        if self.ClusterSendAckQueueObj:
            self.ClusterSendAckQueueObj.ack(messageType, messagePayload)
        else:
            logger.warning("Received 'on_cluster_message_ack' message with no ClusterSendAckQueueObj setup")

    def emit_join_cluster_response(self, SOCKET_IO, serverInfoItems):
        '''Emits 'join_cluster_response' message to primary timer.'''
        payload = {
            'server_info': json.dumps(serverInfoItems)
        }
        self.emit_cluster_msg_to_primary(SOCKET_IO, 'join_cluster_response', payload, False)

    def has_joined_cluster(self):
        return True if self.ClusterSendAckQueueObj else False

    def init_repeater(self):
        self.Events.on(Evt.ALL, 'cluster', self.event_repeater, priority=75, unique=True)

    def event_repeater(self, args):
        try:
            # if there are cluster timers interested in events then emit it out to them
            if self.hasRecEventsSecondaries():
                payload = { 'evt_name': args['_eventName'] }
                del args['_eventName']
                payload['evt_args'] = json.dumps(args, default=lambda _: '<not serializiable>')
                self.emitEventTrigger(payload)
        except Exception as ex:
            logger.exception("Exception in 'Events.trigger()': " + ex)

    def shutdown(self):
        for secondary in self.secondaries:
            secondary.runningFlag = False

    def addSecondary(self, secondary):
        self.secondaries.append(secondary)
        if not secondary.isMirrorMode:  # secondary time in 'split' or 'action' mode
            self.splitSecondaries.append(secondary)
        if secondary.recEventsFlag:
            self.recEventsSecondaries.append(secondary)
        secondary.parentNodeSet = self

    def hasSecondaries(self):
        return (len(self.secondaries) > 0)

    def hasRecEventsSecondaries(self):
        return (len(self.recEventsSecondaries) > 0)

    # return True if secondary is 'split' mode and is or has been connected
    def isSplitSecondaryAvailable(self, secondary_index):
        return (secondary_index < len(self.secondaries)) and \
                    self.secondaries[secondary_index].isSplitMode and \
                    (self.secondaries[secondary_index].lastContactTime > 0 or \
                     self.secondaries[secondary_index].numDisconnects > 0)

    def getSecondaryForIdVal(self, idVal):
        for secondary in self.secondaries:
            if secondary.id == idVal:
                return secondary
        return None
    
    def retrySecondary(self, secondary_id):
        secondary = self.getSecondaryForIdVal(secondary_id)
        if (secondary):
            if not secondary.runningFlag:
                logger.info("Retrying connection to secondary {} at {}".format(secondary.id+1, secondary.address))
                secondary.start_connection()
            else:
                logger.error("Attempted retry of running secondary {} at {} in ClusterNodeSet 'retrySecondary()'".\
                             format(secondary_id+1, secondary.address))
        else:
            logger.error("Secondary ID value ({}) out of bounds in ClusterNodeSet 'retrySecondary()'".\
                         format(secondary_id+1))

    def emit(self, event, data = None):
        for secondary in self.secondaries:
            gevent.spawn(secondary.emit, event, data)

    def emitToSplits(self, event, data = None):
        for secondary in self.splitSecondaries:
            gevent.spawn(secondary.emit, event, data)

    def emitEventTrigger(self, data = None):
        for secondary in self.recEventsSecondaries:
            gevent.spawn(secondary.emit, 'cluster_event_trigger', data)

    def getClusterStatusInfo(self):
        nowTime = monotonic()
        payload = []
        for secondary in self.secondaries:
            upTimeSecs = int(round(nowTime - secondary.firstContactTime)) if secondary.lastContactTime > 0 else 0
            downTimeSecs = int(round(secondary.secsSinceDisconnect)) if secondary.lastContactTime <= 0 else 0
            totalUpSecs = secondary.totalUpTimeSecs + upTimeSecs
            totalDownSecs = secondary.totalDownTimeSecs + downTimeSecs
            if secondary.lastContactTime >= 0:
                lastContactStr = str(int(nowTime-secondary.lastContactTime))
            else:
                if secondary.numDisconnects > 0:
                    lastContactStr = self.__("connection lost")
                else:
                    if secondary.runningFlag:
                        lastContactStr = self.__("never connected")
                    else:
                        lastContactStr = "<button class=\"retry_secondary\" data-secondary_id=\"" + \
                                str(secondary.id) + "\">" + self.__("Not found - click to retry") + "</button>"
            payload.append(
                {'address': secondary.address, \
                 'modeIndicator': (('M' if secondary.isMirrorMode else 'S') if not secondary.isActionMode else 'A'), \
                 'minLatencyMs':  secondary.latencyAveragerObj.minVal, \
                 'avgLatencyMs': secondary.latencyAveragerObj.getIntAvgVal(), \
                 'maxLatencyMs': secondary.latencyAveragerObj.maxVal, \
                 'lastLatencyMs': secondary.latencyAveragerObj.lastVal, \
                 'numDisconnects': secondary.numDisconnects, \
                 'numContacts': secondary.numContacts, \
                 'timeDiffMs': secondary.timeDiffMedianMs, \
                 'upTimeSecs': upTimeSecs, \
                 'downTimeSecs': downTimeSecs, \
                 'availability': round((100.0*totalUpSecs/(totalUpSecs+totalDownSecs) \
                                       if totalUpSecs+totalDownSecs > 0 else 0), 1), \
                 'last_contact': lastContactStr
                 })
        return {'secondaries': payload}

    def doClusterRaceStart(self):
        for secondary in self.secondaries:
            secondary.numDisconnsDuringRace = 0
            if secondary.lastContactTime > 0:
                logger.info("Connected at race start to " + secondary.get_log_str())
                if abs(secondary.timeDiffMedianMs) > SecondaryNode.TIMEDIFF_CORRECTION_THRESH_MS:
                    secondary.timeCorrectionMs = secondary.timeDiffMedianMs
                    logger.info("Secondary {0} clock not synchronized with primary, timeDiff={1}ms".\
                                format(secondary.id+1, secondary.timeDiffMedianMs))
                else:
                    secondary.timeCorrectionMs = 0
                    logger.debug("Secondary {0} clock synchronized OK with primary, timeDiff={1}ms".\
                                 format(secondary.id+1, secondary.timeDiffMedianMs))
            elif secondary.numDisconnects > 0:
                logger.warning("Secondary {0} not connected at race start".format(secondary.id+1))

    def doClusterRaceStop(self):
        for secondary in self.secondaries:
            if secondary.lastContactTime > 0:
                logger.info("Connected at race stop to " + secondary.get_log_str(stoppedRaceFlag=True))
            elif secondary.numDisconnects > 0:
                logger.warning("Not connected at race stop to " + secondary.get_log_str(stoppedRaceFlag=True))

    def __(self, *args, **kwargs):
        return self._Language.__(*args, **kwargs)
